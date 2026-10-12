//! Bounded two-stage transfers. A source file is spooled, then read over RPC in raw
//! binary blocks. One outstanding block per transfer provides simple backpressure.
use crate::{config::safe_filename,error::{BridgeError,Result},node::{BrowserEvent,Node,Reply}};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{collections::HashMap,path::PathBuf,sync::{Arc,Mutex},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use tokio::{fs::{self,File},io::{AsyncReadExt,AsyncSeekExt,AsyncWriteExt},sync::{Mutex as AsyncMutex,Semaphore}};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[derive(Debug,Clone,Serialize)]pub struct JobView{
    pub id:Uuid,pub filename:String,pub direction:String,pub target:String,pub status:String,pub stage:String,
    pub received_bytes:u64,pub total_bytes:Option<u64>,pub sha256:Option<String>,pub error:Option<String>,pub created_at:u64,
}
struct Job{view:JobView,cancel:CancellationToken}
#[derive(Debug,Clone)]pub struct Artifact{pub path:PathBuf,pub filename:String,pub size:u64,pub sha256:String,pub owned:bool}
struct Upload{file:Option<File>,path:PathBuf,filename:String,total:u64,written:u64,expected:Option<String>,hash:Sha256,last:Instant,cancel:CancellationToken,_budget:Budget,_slot:tokio::sync::OwnedSemaphorePermit}
struct Budget{reserved:Arc<Mutex<u64>>,bytes:u64}
impl Drop for Budget{fn drop(&mut self){let mut n=self.reserved.lock().unwrap();*n=n.saturating_sub(self.bytes);}}
pub struct Transfers{
    jobs:Mutex<HashMap<Uuid,Job>>,files:Mutex<HashMap<Uuid,Artifact>>,uploads:Mutex<HashMap<Uuid,Arc<AsyncMutex<Upload>>>>,
    reserved:Arc<Mutex<u64>>,slots:Arc<Semaphore>,upload_slots:Arc<Semaphore>,
}
impl Default for Transfers{fn default()->Self{Self{jobs:Mutex::new(HashMap::new()),files:Mutex::new(HashMap::new()),uploads:Mutex::new(HashMap::new()),reserved:Arc::new(Mutex::new(0)),slots:Arc::new(Semaphore::new(2)),upload_slots:Arc::new(Semaphore::new(2))}}}
impl Transfers{
    fn new_job(&self,name:String,direction:&str,target:&str)->Result<(Uuid,CancellationToken)>{
        let mut jobs=self.jobs.lock().unwrap();
        if jobs.len()>=200{return Err(BridgeError::new("BUSY","200 transfer records reached; remove completed transfers before adding more"))}
        let id=Uuid::new_v4();let cancel=CancellationToken::new();
        jobs.insert(id,Job{view:JobView{id,filename:name,direction:direction.into(),target:target.into(),status:"queued".into(),stage:"queued".into(),received_bytes:0,total_bytes:None,sha256:None,error:None,created_at:SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()as u64},cancel:cancel.clone()});Ok((id,cancel))
    }
    pub fn list(&self)->Vec<JobView>{let mut list:Vec<_>=self.jobs.lock().unwrap().values().map(|j|j.view.clone()).collect();list.sort_by_key(|j|std::cmp::Reverse(j.created_at));list}
    pub fn get(&self,id:Uuid)->Result<JobView>{self.jobs.lock().unwrap().get(&id).map(|j|j.view.clone()).ok_or_else(||BridgeError::new("NOT_FOUND","Transfer not found"))}
    pub fn artifact(&self,id:Uuid)->Result<Artifact>{self.files.lock().unwrap().get(&id).cloned().ok_or_else(||BridgeError::new("NOT_FOUND","Completed file handle not found"))}
    fn update(&self,id:Uuid,f:impl FnOnce(&mut JobView)){if let Some(j)=self.jobs.lock().unwrap().get_mut(&id){f(&mut j.view)}}
    fn failure(&self,id:Uuid,e:&BridgeError){self.update(id,|j|{j.status=if e.code=="CANCELLED"{"cancelled"}else{"failed"}.into();j.error=Some(format!("{}: {}",e.code,e.message));})}
    fn register(&self,id:Uuid,artifact:Artifact)->Result<()>{let mut f=self.files.lock().unwrap();if f.len()>=200&&!f.contains_key(&id){return Err(BridgeError::new("BUSY","File handle limit reached"))}f.insert(id,artifact);Ok(())}
    pub fn cancel(&self,id:Uuid)->Result<()>{let jobs=self.jobs.lock().unwrap();let j=jobs.get(&id).ok_or_else(||BridgeError::new("NOT_FOUND","Transfer not found"))?;if ["completed","failed","cancelled"].contains(&j.view.status.as_str()){return Err(BridgeError::invalid("Transfer is already finished"))}j.cancel.cancel();Ok(())}
}
fn id_param(p:&Value)->Result<Uuid>{Uuid::parse_str(p["id"].as_str().unwrap_or("")).map_err(|_|BridgeError::invalid("Expected transfer id"))}
fn u64_param(p:&Value,k:&str)->Result<u64>{p[k].as_u64().ok_or_else(||BridgeError::invalid(format!("Expected {k}")))}
fn cancelled()->BridgeError{BridgeError::new("CANCELLED","Transfer cancelled")}
impl Node{
    async fn budget(&self,bytes:u64)->Result<Budget>{
        let cfg=self.cfg();if bytes>cfg.max_file_bytes{return Err(BridgeError::new("FILE_TOO_LARGE","File exceeds configured size limit"))}
        let mut used=0u64;let mut entries=fs::read_dir(&cfg.download_dir).await?;
        while let Some(entry)=entries.next_entry().await?{let meta=entry.metadata().await?;if meta.is_file(){used=used.saturating_add(meta.len())}}
        let mut r=self.transfers.reserved.lock().unwrap();
        // Conservative: partial files may be counted twice. This errs toward rejecting, not overfilling disk.
        if used.saturating_add(*r).saturating_add(bytes)>cfg.max_spool_bytes{return Err(BridgeError::new("SPOOL_QUOTA","Download directory quota reached"))}
        *r+=bytes;Ok(Budget{reserved:self.transfers.reserved.clone(),bytes})
    }
    pub fn start_source_download(self:&Arc<Self>,p:Value)->Result<Reply>{
        let filename=safe_filename(p["filename"].as_str().unwrap_or("download.bin"))?;
        let executor=p["executor"].as_str().unwrap_or("http").to_string();
        if !["http","browser"].contains(&executor.as_str()){return Err(BridgeError::invalid("executor must be http or browser"))}
        let request=p.get("request").cloned().unwrap_or_else(||json!({"url":p["url"],"method":p.get("method").and_then(Value::as_str).unwrap_or("GET"),"headers":p.get("headers").cloned().unwrap_or(json!({}))}));
        crate::http_executor::validate(&self.cfg(),&request)?;
        let permit=self.transfers.slots.clone().try_acquire_owned().map_err(|_|BridgeError::new("BUSY","At most two transfer jobs per node"))?;
        let(id,cancel)=self.transfers.new_job(filename.clone(),"source","local")?;let node=self.clone();
        tokio::spawn(async move{let _permit=permit;
            let result=tokio::select!{_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=node.spool_source(id,filename,&executor,request)=>r};
            if let Err(e)=result{node.transfers.failure(id,&e);let _=fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;node.log("error","transfer",e.code,id);}
        });Ok(Reply::json(serde_json::to_value(self.transfers.get(id)?)?))
    }
    async fn spool_source(self:&Arc<Self>,id:Uuid,filename:String,executor:&str,params:Value)->Result<()> {
        let cfg=self.cfg();let _budget=self.budget(cfg.max_file_bytes).await?;
        let path=cfg.download_dir.join(format!("{id}.part"));let mut file=File::options().write(true).create_new(true).open(&path).await?;
        let mut hash=Sha256::new();let mut received=0u64;
        self.transfers.update(id,|j|{j.status="running".into();j.stage="source_download".into()});
        if executor=="browser"{
            let mut task=self.begin_browser("browser.download",params).await?;
            let mut began=false;
            loop{match task.next().await?{
                BrowserEvent::Meta(v)=>match v["type"].as_str(){
                    Some("download.begin")=>{if began{return Err(BridgeError::invalid("Duplicate download begin"))}began=true;let total=v["total_bytes"].as_u64();if total.is_some_and(|n|n>cfg.max_file_bytes){return Err(BridgeError::new("FILE_TOO_LARGE","Browser download exceeds file limit"))}self.transfers.update(id,|j|j.total_bytes=total);},
                    Some("download.end")=>{if !began||v["total_bytes"].as_u64()!=Some(received){return Err(BridgeError::invalid("Browser download length mismatch"))}break},
                    Some("task.error")=>return Err(BridgeError::new("BROWSER_ERROR",v["message"].as_str().unwrap_or("Browser download failed"))),
                    _=>return Err(BridgeError::invalid("Unexpected browser download message")),
                },
                BrowserEvent::Chunk(meta,data)=>{
                    if !began||meta["type"]!="chunk"||meta["offset"].as_u64()!=Some(received)||data.is_empty(){return Err(BridgeError::invalid("Browser chunk offset mismatch"))}
                    received+=data.len()as u64;if received>cfg.max_file_bytes{return Err(BridgeError::new("FILE_TOO_LARGE","Browser download exceeds file limit"))}
                    file.write_all(&data).await?;hash.update(&data);self.transfers.update(id,|j|j.received_bytes=received);task.ack(received)?;
                }
            }}
        }else{
            let res=crate::http_executor::response(self,&params).await?;
            if !res.status().is_success(){return Err(BridgeError::new("HTTP_ERROR",format!("Download returned HTTP {}",res.status().as_u16())))}
            let total=res.content_length();if total.is_some_and(|n|n>cfg.max_file_bytes){return Err(BridgeError::new("FILE_TOO_LARGE","HTTP download exceeds file limit"))}
            self.transfers.update(id,|j|j.total_bytes=total);let mut stream=res.bytes_stream();
            while let Some(chunk)=stream.next().await{let data=chunk.map_err(|_|BridgeError::new("HTTP_ERROR","Download stream interrupted"))?;received+=data.len()as u64;if received>cfg.max_file_bytes{return Err(BridgeError::new("FILE_TOO_LARGE","Download exceeds file limit"))}file.write_all(&data).await?;hash.update(&data);self.transfers.update(id,|j|j.received_bytes=received);}
        }
        file.flush().await?;file.sync_all().await?;drop(file);
        let sha=format!("{:x}",hash.finalize());let final_path=cfg.download_dir.join(format!("{id}-{filename}"));fs::rename(path,&final_path).await?;
        self.transfers.register(id,Artifact{path:final_path,filename,size:received,sha256:sha.clone(),owned:true})?;
        self.transfers.update(id,|j|{j.status="completed".into();j.stage="completed".into();j.total_bytes=Some(received);j.sha256=Some(sha)});self.log("info","transfer","Source file ready and SHA-256 calculated",id);Ok(())
    }
    pub fn transfer_status_reply(&self,p:Value)->Result<Reply>{Ok(Reply::json(serde_json::to_value(self.transfers.get(id_param(&p)?)?)?))}
    pub fn cancel_transfer_reply(&self,p:Value)->Result<Reply>{self.transfers.cancel(id_param(&p)?)?;Ok(Reply::json(json!({"cancelling":true})))}
    pub fn start_download(self:&Arc<Self>,p:Value)->Result<Value>{
        let target=p["target"].as_str().unwrap_or("peer");
        if target=="local"{return Ok(self.start_source_download(p)?.value)}
        if target!="peer"{return Err(BridgeError::invalid("target must be local or peer"))}
        if !self.peer.lock().unwrap().as_ref().is_some_and(|p|p.ready){return Err(BridgeError::new("PEER_DISCONNECTED","Peer is offline"))}
        let filename=safe_filename(p["filename"].as_str().unwrap_or("download.bin"))?;
        let permit=self.transfers.slots.clone().try_acquire_owned().map_err(|_|BridgeError::new("BUSY","At most two transfer jobs per node"))?;
        let(id,cancel)=self.transfers.new_job(filename.clone(),"receive","peer")?;let node=self.clone();
        tokio::spawn(async move{let _permit=permit;let mut remote=None;
            let result=tokio::select!{_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
                let started=node.peer_call("download.start",p,Bytes::new(),60000).await?;let rid=id_param(&started.value)?;remote=Some(rid);
                node.transfers.update(id,|j|{j.status="running".into();j.stage="source_download".into()});
                loop{let r=node.peer_call("download.status",json!({"id":rid}),Bytes::new(),60000).await?;
                    match r.value["status"].as_str(){Some("completed")=>break,Some("failed")|Some("cancelled")=>return Err(BridgeError::new("SOURCE_FAILED",r.value["error"].as_str().unwrap_or("Remote download failed"))),_=>{}}
                    node.transfers.update(id,|j|{j.received_bytes=r.value["received_bytes"].as_u64().unwrap_or(0);j.total_bytes=r.value["total_bytes"].as_u64()});tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let metadata=node.peer_call("download.status",json!({"id":rid}),Bytes::new(),60000).await?;
                let total=metadata.value["total_bytes"].as_u64().ok_or_else(||BridgeError::invalid("Missing source size"))?;let hash=metadata.value["sha256"].as_str().ok_or_else(||BridgeError::invalid("Missing source SHA-256"))?.to_string();
                node.receive_file(id,rid,filename,total,&hash).await
            }=>r};
            if let Err(e)=result{node.transfers.failure(id,&e);let _=fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;if let Some(rid)=remote{let _=node.peer_call("download.cancel",json!({"id":rid}),Bytes::new(),3000).await;}node.log("error","transfer",e.code,id);}
            // Source artifacts intentionally remain available until explicit deletion; never silently delete a peer's file.
        });Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    async fn receive_file(self:&Arc<Self>,id:Uuid,remote:Uuid,filename:String,total:u64,expected:&str)->Result<()> {
        let cfg=self.cfg();let _budget=self.budget(total).await?;let path=cfg.download_dir.join(format!("{id}.part"));
        let mut file=File::options().write(true).create_new(true).open(&path).await?;let mut hash=Sha256::new();let mut offset=0u64;
        self.transfers.update(id,|j|{j.status="running".into();j.stage="serial_transfer".into();j.received_bytes=0;j.total_bytes=Some(total)});
        while offset<total {
            let n=(total-offset).min(cfg.chunk_bytes as u64);
            let r=self.peer_call("file.read",json!({"id":remote,"offset":offset,"length":n}),Bytes::new(),60000).await?;
            if r.data.len()as u64!=n||r.value["offset"].as_u64()!=Some(offset){return Err(BridgeError::invalid("Peer returned an invalid file block"))}
            file.write_all(&r.data).await?;hash.update(&r.data);offset+=n;self.transfers.update(id,|j|j.received_bytes=offset);
        }
        let actual=format!("{:x}",hash.finalize());if actual!=expected{return Err(BridgeError::new("HASH_MISMATCH","SHA-256 verification failed; partial file is not published"))}
        file.flush().await?;file.sync_all().await?;drop(file);let final_path=cfg.download_dir.join(format!("{id}-{filename}"));fs::rename(&path,&final_path).await?;
        self.transfers.register(id,Artifact{path:final_path,filename,size:total,sha256:actual.clone(),owned:true})?;
        self.transfers.update(id,|j|{j.status="completed".into();j.stage="completed".into();j.sha256=Some(actual)});self.log("info","transfer","File received; SHA-256 matches",id);Ok(())
    }
    pub async fn open_shared(&self,p:Value)->Result<Reply>{
        let filename=safe_filename(p["filename"].as_str().unwrap_or(""))?;let root=fs::canonicalize(self.cfg().shared_dir).await?;let path=fs::canonicalize(root.join(&filename)).await?;
        if !path.starts_with(&root){return Err(BridgeError::new("FORBIDDEN","File is outside the shared directory"))}
        let m=fs::metadata(&path).await?;if !m.is_file()||m.len()>self.cfg().max_file_bytes{return Err(BridgeError::invalid("Not a regular file, or file is too large"))}
        let hash=hash_file(&path).await?;let id=Uuid::new_v4();self.transfers.register(id,Artifact{path,filename:filename.clone(),size:m.len(),sha256:hash.clone(),owned:false})?;
        Ok(Reply::json(json!({"id":id,"filename":filename,"total_bytes":m.len(),"sha256":hash})))
    }
    pub async fn read_block(&self,p:Value)->Result<Reply>{
        let a=self.transfers.artifact(id_param(&p)?)?;let offset=u64_param(&p,"offset")?;let len=u64_param(&p,"length")?;
        if len==0||len>16384||offset>a.size||len>a.size-offset{return Err(BridgeError::invalid("Invalid file block range (max 16 KiB)"))}
        let mut file=File::open(a.path).await?;file.seek(std::io::SeekFrom::Start(offset)).await?;let mut data=vec![0;len as usize];file.read_exact(&mut data).await?;Ok(Reply{value:json!({"offset":offset}),data:Bytes::from(data)})
    }
    pub fn start_pull(self:&Arc<Self>,p:Value)->Result<Value>{
        let filename=safe_filename(p["filename"].as_str().unwrap_or(""))?;
        let permit=self.transfers.slots.clone().try_acquire_owned().map_err(|_|BridgeError::new("BUSY","Transfer concurrency limit"))?;
        let(id,cancel)=self.transfers.new_job(filename.clone(),"receive","peer")?;let node=self.clone();
        tokio::spawn(async move{let _permit=permit;let result=tokio::select!{_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
            let opened=node.peer_call("file.open",json!({"filename":filename}),Bytes::new(),60000).await?;
            let remote=id_param(&opened.value)?;let total=u64_param(&opened.value,"total_bytes")?;let hash=opened.value["sha256"].as_str().ok_or_else(||BridgeError::invalid("Missing SHA-256"))?;
            let result=node.receive_file(id,remote,filename,total,hash).await;
            let _=node.peer_call("file.remove",json!({"id":remote}),Bytes::new(),3000).await;result
        }=>r};if let Err(e)=result{node.transfers.failure(id,&e);let _=fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;}});
        Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    pub async fn create_upload(&self,p:Value)->Result<Reply>{
        let filename=safe_filename(p["filename"].as_str().unwrap_or(""))?;let total=u64_param(&p,"total_bytes")?;
        let slot=self.transfers.upload_slots.clone().try_acquire_owned().map_err(|_|BridgeError::new("BUSY","At most two uploads"))?;
        let budget=self.budget(total).await?;let local=p["local_staging"].as_bool().unwrap_or(false);let (id,cancel)=self.transfers.new_job(filename.clone(),if local{"staging"}else{"receive"},if local{"local"}else{"peer"})?;self.transfers.update(id,|j|{j.status="running".into();j.stage=if local{"local_staging"}else{"serial_transfer"}.into();j.total_bytes=Some(total)});let path=self.cfg().download_dir.join(format!("{id}.part"));
        let file=File::options().create_new(true).write(true).open(&path).await?;
        let upload=Upload{file:Some(file),path,filename,total,written:0,expected:p["sha256"].as_str().map(str::to_owned),hash:Sha256::new(),last:Instant::now(),cancel,_budget:budget,_slot:slot};
        self.transfers.uploads.lock().unwrap().insert(id,Arc::new(AsyncMutex::new(upload)));
        Ok(Reply::json(json!({"id":id,"offset":0})))
    }
    pub async fn write_block(&self,p:Value,data:Bytes)->Result<Reply>{
        let id=id_param(&p)?;let offset=u64_param(&p,"offset")?;let upload=self.transfers.uploads.lock().unwrap().get(&id).cloned().ok_or_else(||BridgeError::new("NOT_FOUND","Upload not found"))?;
        let mut u=upload.lock().await;
        if u.cancel.is_cancelled(){return Err(cancelled())}
        if offset!=u.written||data.len()>16384||data.is_empty()||data.len()as u64>u.total-u.written{return Err(BridgeError::invalid("Invalid upload block size or offset"))}
        u.file.as_mut().ok_or_else(||BridgeError::invalid("Upload closed"))?.write_all(&data).await?;u.hash.update(&data);u.written+=data.len()as u64;u.last=Instant::now();self.transfers.update(id,|j|j.received_bytes=u.written);Ok(Reply::json(json!({"offset":u.written})))
    }
    pub async fn finish_upload(&self,p:Value)->Result<Reply>{
        let id=id_param(&p)?;let upload=self.transfers.uploads.lock().unwrap().remove(&id).ok_or_else(||BridgeError::new("NOT_FOUND","Upload not found"))?;let mut u=upload.lock().await;
        let actual=format!("{:x}",u.hash.clone().finalize());
        if u.cancel.is_cancelled(){u.file.take();let _=fs::remove_file(&u.path).await;self.transfers.failure(id,&cancelled());return Err(cancelled())}
        if u.written!=u.total||u.expected.as_ref().is_some_and(|e|e!=&actual){u.file.take();let _=fs::remove_file(&u.path).await;let e=BridgeError::new("HASH_MISMATCH","Upload size or SHA-256 mismatch");self.transfers.failure(id,&e);return Err(e)}
        let mut file=u.file.take().ok_or_else(||BridgeError::invalid("Upload already closed"))?;file.flush().await?;file.sync_all().await?;drop(file);
        let final_path=self.cfg().download_dir.join(format!("{id}-{}",u.filename));fs::rename(&u.path,&final_path).await?;
        self.transfers.register(id,Artifact{path:final_path,filename:u.filename.clone(),size:u.total,sha256:actual.clone(),owned:true})?;
        self.transfers.update(id,|j|{j.status="completed".into();j.stage="completed".into();j.received_bytes=u.total;j.sha256=Some(actual.clone())});self.log("info","transfer","Incoming upload complete; checksum verified",id);
        Ok(Reply::json(json!({"id":id,"filename":u.filename,"total_bytes":u.total,"sha256":actual})))
    }
    pub fn start_push(self:&Arc<Self>,artifact:Uuid)->Result<Value>{
        let source=self.transfers.artifact(artifact)?;let permit=self.transfers.slots.clone().try_acquire_owned().map_err(|_|BridgeError::new("BUSY","Transfer concurrency limit"))?;
        let(id,cancel)=self.transfers.new_job(source.filename.clone(),"send","peer")?;let node=self.clone();
        tokio::spawn(async move{let _permit=permit;let mut remote=None;let result=tokio::select!{_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
            let r=node.peer_call("file.create",json!({"filename":source.filename,"total_bytes":source.size,"sha256":source.sha256}),Bytes::new(),60000).await?;let rid=id_param(&r.value)?;remote=Some(rid);let mut file=File::open(&source.path).await?;let mut offset=0;
            node.transfers.update(id,|j|{j.status="running".into();j.stage="serial_transfer".into();j.total_bytes=Some(source.size)});
            while offset<source.size{let n=(source.size-offset).min(node.cfg().chunk_bytes as u64)as usize;let mut data=vec![0;n];file.read_exact(&mut data).await?;let r=node.peer_call("file.write",json!({"id":rid,"offset":offset}),Bytes::from(data),60000).await?;offset+=n as u64;if r.value["offset"].as_u64()!=Some(offset){return Err(BridgeError::invalid("Upload acknowledgement mismatch"))}node.transfers.update(id,|j|j.received_bytes=offset);}
            let result=node.peer_call("file.finish",json!({"id":rid}),Bytes::new(),60000).await?;if result.value["sha256"].as_str()!=Some(source.sha256.as_str()){return Err(BridgeError::new("HASH_MISMATCH","Peer checksum mismatch"))}
            node.transfers.update(id,|j|{j.status="completed".into();j.stage="completed".into();j.sha256=Some(source.sha256.clone())});Ok(())
        }=>r};if let Err(e)=result{node.transfers.failure(id,&e);if let Some(rid)=remote{let _=node.peer_call("file.remove",json!({"id":rid}),Bytes::new(),3000).await;}}});
        Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    pub async fn remove_artifact(&self,p:Value)->Result<Reply>{
        let id=id_param(&p)?;
        let upload={self.transfers.uploads.lock().unwrap().remove(&id)};
        if let Some(u)=upload{let mut u=u.lock().await;u.file.take();let _=fs::remove_file(&u.path).await;self.transfers.failure(id,&cancelled());}
        let artifact={self.transfers.files.lock().unwrap().remove(&id)};
        if let Some(a)=artifact{if a.owned{fs::remove_file(a.path).await?;}}
        {let mut jobs=self.transfers.jobs.lock().unwrap();if jobs.get(&id).is_some_and(|j|["completed","failed","cancelled"].contains(&j.view.status.as_str())){jobs.remove(&id);}}
        Ok(Reply::json(json!({"removed":true})))
    }
    pub async fn reap_uploads(&self){
        let uploads:Vec<_>=self.transfers.uploads.lock().unwrap().iter().map(|(id,u)|(*id,u.clone())).collect();
        for(id,u)in uploads{let expired={let u=u.lock().await;u.cancel.is_cancelled()||u.last.elapsed()>Duration::from_secs(300)};if expired{let _=self.remove_artifact(json!({"id":id})).await;}}
    }
}
async fn hash_file(path:&PathBuf)->Result<String>{let mut file=File::open(path).await?;let mut h=Sha256::new();let mut buf=vec![0;65536];loop{let n=file.read(&mut buf).await?;if n==0{break}h.update(&buf[..n]);}Ok(format!("{:x}",h.finalize()))}
#[cfg(test)]mod tests{
    use super::*;
    #[tokio::test]async fn upload_enforces_offsets_and_hash(){let dir=tempfile::tempdir().unwrap();let mut cfg=crate::config::Config::default();cfg.download_dir=dir.path().into();cfg.shared_dir=dir.path().into();let node=Node::new(cfg,dir.path().join("config.toml")).unwrap();let digest=format!("{:x}",Sha256::digest(b"hello"));let start=node.create_upload(json!({"filename":"test.bin","total_bytes":5,"sha256":digest})).await.unwrap();let id=&start.value["id"];
        assert!(node.write_block(json!({"id":id,"offset":1}),Bytes::from_static(b"hello")).await.is_err());node.write_block(json!({"id":id,"offset":0}),Bytes::from_static(b"hello")).await.unwrap();let result=node.finish_upload(json!({"id":id})).await.unwrap();assert_eq!(result.value["sha256"],digest);
    }
    #[tokio::test]async fn empty_file_hash_is_verified(){let dir=tempfile::tempdir().unwrap();let mut cfg=crate::config::Config::default();cfg.download_dir=dir.path().into();let node=Node::new(cfg,dir.path().join("config.toml")).unwrap();let start=node.create_upload(json!({"filename":"empty","total_bytes":0,"sha256":format!("{:x}",Sha256::digest([]))})).await.unwrap();assert!(node.finish_upload(json!({"id":start.value["id"]})).await.is_ok());}
}
