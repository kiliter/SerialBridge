//! Bounded two-stage transfers. A source file is spooled, then read over RPC in raw
//! binary blocks. One outstanding block per transfer provides simple backpressure.
use crate::{
    config::safe_filename,
    error::{BridgeError, Result},
    node::{BrowserEvent, Node, Reply},
};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, Semaphore},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: Uuid,
    pub filename: String,
    pub direction: String,
    pub target: String,
    pub status: String,
    pub stage: String,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub created_at: u64,
}
struct Job {
    view: JobView,
    cancel: CancellationToken,
    peer_session: Option<Uuid>,
}
#[derive(Debug, Clone)]
pub struct Artifact {
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub owned: bool,
    pub peer_session: Option<Uuid>,
}
struct Upload {
    file: Option<File>,
    path: PathBuf,
    filename: String,
    total: u64,
    written: u64,
    expected: Option<String>,
    hash: Sha256,
    last: Instant,
    cancel: CancellationToken,
    _budget: Budget,
    _slot: tokio::sync::OwnedSemaphorePermit,
}
struct Budget {
    reserved: Arc<Mutex<u64>>,
    bytes: u64,
}
impl Drop for Budget {
    fn drop(&mut self) {
        let mut n = self.reserved.lock().unwrap();
        *n = n.saturating_sub(self.bytes);
    }
}
pub struct Transfers {
    jobs: Mutex<HashMap<Uuid, Job>>,
    files: Mutex<HashMap<Uuid, Artifact>>,
    uploads: Mutex<HashMap<Uuid, Arc<AsyncMutex<Upload>>>>,
    reserved: Arc<Mutex<u64>>,
    slots: Arc<Semaphore>,
    upload_slots: Arc<Semaphore>,
}
impl Default for Transfers {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            reserved: Arc::new(Mutex::new(0)),
            slots: Arc::new(Semaphore::new(2)),
            upload_slots: Arc::new(Semaphore::new(2)),
        }
    }
}
impl Transfers {
    fn new_job(
        &self,
        name: String,
        direction: &str,
        target: &str,
    ) -> Result<(Uuid, CancellationToken)> {
        self.new_job_owned(name, direction, target, None)
    }
    fn new_job_owned(
        &self,
        name: String,
        direction: &str,
        target: &str,
        peer_session: Option<Uuid>,
    ) -> Result<(Uuid, CancellationToken)> {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.len() >= 200 {
            return Err(BridgeError::new(
                "BUSY",
                "200 transfer records reached; remove completed transfers before adding more",
            ));
        }
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        jobs.insert(
            id,
            Job {
                peer_session,
                view: JobView {
                    id,
                    filename: name,
                    direction: direction.into(),
                    target: target.into(),
                    status: "queued".into(),
                    stage: "queued".into(),
                    received_bytes: 0,
                    total_bytes: None,
                    sha256: None,
                    error: None,
                    created_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                },
                cancel: cancel.clone(),
            },
        );
        Ok((id, cancel))
    }
    pub fn list(&self) -> Vec<JobView> {
        let mut list: Vec<_> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .map(|j| j.view.clone())
            .collect();
        list.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        list
    }
    pub fn get(&self, id: Uuid) -> Result<JobView> {
        self.jobs
            .lock()
            .unwrap()
            .get(&id)
            .map(|j| j.view.clone())
            .ok_or_else(|| BridgeError::new("NOT_FOUND", "Transfer not found"))
    }
    pub fn artifact(&self, id: Uuid) -> Result<Artifact> {
        self.files
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| BridgeError::new("NOT_FOUND", "Completed file handle not found"))
    }
    fn update(&self, id: Uuid, f: impl FnOnce(&mut JobView)) {
        if let Some(j) = self.jobs.lock().unwrap().get_mut(&id) {
            f(&mut j.view)
        }
    }
    fn failure(&self, id: Uuid, e: &BridgeError) {
        self.update(id, |j| {
            j.status = if e.code == "CANCELLED" {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            j.error = Some(format!("{}: {}", e.code, e.message));
        })
    }
    fn register(&self, id: Uuid, artifact: Artifact) -> Result<()> {
        let mut f = self.files.lock().unwrap();
        if f.len() >= 200 && !f.contains_key(&id) {
            return Err(BridgeError::new("BUSY", "File handle limit reached"));
        }
        f.insert(id, artifact);
        Ok(())
    }
    fn publish(&self, id: Uuid, partial: &std::path::Path, artifact: Artifact) -> Result<()> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs.get_mut(&id).ok_or_else(cancelled)?;
        if job.cancel.is_cancelled() {
            return Err(cancelled());
        }
        let mut files = self.files.lock().unwrap();
        if files.len() >= 200 && !files.contains_key(&id) {
            return Err(BridgeError::new("BUSY", "File handle limit reached"));
        }
        // 文件已 flush/sync；重命名、登记和完成状态之间不允许取消或 await。
        // 先检查容量，避免重命名成功后留下未登记且无法移除的文件。
        std::fs::rename(partial, &artifact.path)?;
        job.view.status = "completed".into();
        job.view.stage = "completed".into();
        job.view.received_bytes = artifact.size;
        job.view.total_bytes = Some(artifact.size);
        job.view.sha256 = Some(artifact.sha256.clone());
        files.insert(id, artifact);
        Ok(())
    }
    pub fn cancel(&self, id: Uuid) -> Result<()> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs
            .get(&id)
            .ok_or_else(|| BridgeError::new("NOT_FOUND", "Transfer not found"))?;
        if ["completed", "failed", "cancelled"].contains(&j.view.status.as_str()) {
            return Err(BridgeError::invalid("Transfer is already finished"));
        }
        j.cancel.cancel();
        Ok(())
    }
    pub(crate) fn cancel_peer(&self, epoch: Uuid) -> Vec<Uuid> {
        let jobs = self.jobs.lock().unwrap();
        let ids = jobs
            .iter()
            .filter_map(|(id, job)| {
                if job.peer_session == Some(epoch)
                    && !["completed", "failed", "cancelled"].contains(&job.view.status.as_str())
                {
                    job.cancel.cancel();
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        drop(jobs);
        // Shared-file handles are session resources; completed owned caches
        // deliberately remain available for explicit removal.
        self.files
            .lock()
            .unwrap()
            .retain(|_, file| file.owned || file.peer_session != Some(epoch));
        ids
    }
}
fn id_param(p: &Value) -> Result<Uuid> {
    Uuid::parse_str(p["id"].as_str().unwrap_or(""))
        .map_err(|_| BridgeError::invalid("Expected transfer id"))
}
fn u64_param(p: &Value, k: &str) -> Result<u64> {
    p[k].as_u64()
        .ok_or_else(|| BridgeError::invalid(format!("Expected {k}")))
}
fn cancelled() -> BridgeError {
    BridgeError::new("CANCELLED", "Transfer cancelled")
}
impl Node {
    fn peer_session_active(&self, session: Option<Uuid>) -> bool {
        match session {
            None => true,
            Some(epoch) => self
                .peer
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|p| p.ready && p.epoch == epoch),
        }
    }
    async fn budget(&self, bytes: u64) -> Result<Budget> {
        let cfg = self.cfg();
        if bytes > cfg.max_file_bytes {
            return Err(BridgeError::new(
                "FILE_TOO_LARGE",
                "File exceeds configured size limit",
            ));
        }
        let mut used = 0u64;
        let mut entries = fs::read_dir(&cfg.download_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let meta = match entry.metadata().await {
                Ok(meta) => meta,
                // Another task can remove/rename a part between read_dir and
                // metadata. A vanished entry occupies no space in this scan.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if meta.is_file() {
                used = used.saturating_add(meta.len())
            }
        }
        let mut r = self.transfers.reserved.lock().unwrap();
        // Conservative: partial files may be counted twice. This errs toward rejecting, not overfilling disk.
        if used.saturating_add(*r).saturating_add(bytes) > cfg.max_spool_bytes {
            return Err(BridgeError::new(
                "SPOOL_QUOTA",
                "Download directory quota reached",
            ));
        }
        *r += bytes;
        Ok(Budget {
            reserved: self.transfers.reserved.clone(),
            bytes,
        })
    }
    pub fn start_source_download(self: &Arc<Self>, p: Value) -> Result<Reply> {
        self.start_source_download_owned(p, None)
    }
    pub(crate) fn start_source_download_owned(
        self: &Arc<Self>,
        p: Value,
        peer_session: Option<Uuid>,
    ) -> Result<Reply> {
        let filename = safe_filename(p["filename"].as_str().unwrap_or("download.bin"))?;
        let executor = p["executor"].as_str().unwrap_or("http").to_string();
        if !["http", "browser"].contains(&executor.as_str()) {
            return Err(BridgeError::invalid("executor must be http or browser"));
        }
        let request=p.get("request").cloned().unwrap_or_else(||json!({"url":p["url"],"method":p.get("method").and_then(Value::as_str).unwrap_or("GET"),"headers":p.get("headers").cloned().unwrap_or(json!({}))}));
        crate::http_executor::validate(&self.cfg(), &request)?;
        let permit = self
            .transfers
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new("BUSY", "At most two transfer jobs per node"))?;
        let (id, cancel) =
            self.transfers
                .new_job_owned(filename.clone(), "source", "local", peer_session)?;
        // Register first, then check the session: disconnect cleanup either
        // sees this job, or this check observes that the session already ended.
        if !self.peer_session_active(peer_session) {
            cancel.cancel();
            self.transfers.failure(id, &cancelled());
            return Err(BridgeError::new(
                "PEER_DISCONNECTED",
                "Peer session closed during download creation",
            ));
        }
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::select! {_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=node.spool_source(id,filename,&executor,request)=>r};
            if let Err(e) = result {
                node.transfers.failure(id, &e);
                let _ = fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;
                node.log("error", "transfer", e.code, id);
            }
        });
        Ok(Reply::json(serde_json::to_value(self.transfers.get(id)?)?))
    }
    async fn spool_source(
        self: &Arc<Self>,
        id: Uuid,
        filename: String,
        executor: &str,
        params: Value,
    ) -> Result<()> {
        let cfg = self.cfg();
        let _budget = self.budget(cfg.max_file_bytes).await?;
        let path = cfg.download_dir.join(format!("{id}.part"));
        let mut file = File::from_std(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?,
        );
        let mut hash = Sha256::new();
        let mut received = 0u64;
        self.transfers.update(id, |j| {
            j.status = "running".into();
            j.stage = "source_download".into()
        });
        if executor == "browser" {
            let mut task = self.begin_browser("browser.download", params).await?;
            let mut began = false;
            let mut declared = None;
            loop {
                match task.next().await? {
                    BrowserEvent::Meta(v) => match v["type"].as_str() {
                        Some("download.begin") => {
                            if began {
                                return Err(BridgeError::invalid("Duplicate download begin"));
                            }
                            began = true;
                            let total = v["total_bytes"].as_u64();
                            declared = total;
                            if total.is_some_and(|n| n > cfg.max_file_bytes) {
                                return Err(BridgeError::new(
                                    "FILE_TOO_LARGE",
                                    "Browser download exceeds file limit",
                                ));
                            }
                            self.transfers.update(id, |j| j.total_bytes = total);
                        }
                        Some("download.end") => {
                            if !began
                                || v["total_bytes"].as_u64() != Some(received)
                                || declared.is_some_and(|total| total != received)
                            {
                                return Err(BridgeError::invalid(
                                    "Browser download length mismatch",
                                ));
                            }
                            break;
                        }
                        Some("task.error") => {
                            return Err(BridgeError::new(
                                "BROWSER_ERROR",
                                v["message"].as_str().unwrap_or("Browser download failed"),
                            ))
                        }
                        _ => {
                            return Err(BridgeError::invalid("Unexpected browser download message"))
                        }
                    },
                    BrowserEvent::Chunk(meta, data) => {
                        if !began
                            || meta["type"] != "chunk"
                            || meta["offset"].as_u64() != Some(received)
                            || data.is_empty()
                        {
                            return Err(BridgeError::invalid("Browser chunk offset mismatch"));
                        }
                        received += data.len() as u64;
                        if declared.is_some_and(|total| received > total) {
                            return Err(BridgeError::invalid("Browser download length mismatch"));
                        }
                        if received > cfg.max_file_bytes {
                            return Err(BridgeError::new(
                                "FILE_TOO_LARGE",
                                "Browser download exceeds file limit",
                            ));
                        }
                        file.write_all(&data).await?;
                        hash.update(&data);
                        self.transfers.update(id, |j| j.received_bytes = received);
                        task.ack(received)?;
                    }
                }
            }
        } else {
            let res = crate::http_executor::response(self, &params).await?;
            if !res.status().is_success() {
                return Err(BridgeError::new(
                    "HTTP_ERROR",
                    format!("Download returned HTTP {}", res.status().as_u16()),
                ));
            }
            let total = res.content_length();
            if total.is_some_and(|n| n > cfg.max_file_bytes) {
                return Err(BridgeError::new(
                    "FILE_TOO_LARGE",
                    "HTTP download exceeds file limit",
                ));
            }
            self.transfers.update(id, |j| j.total_bytes = total);
            let mut stream = res.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let data = chunk
                    .map_err(|_| BridgeError::new("HTTP_ERROR", "Download stream interrupted"))?;
                received += data.len() as u64;
                if received > cfg.max_file_bytes {
                    return Err(BridgeError::new(
                        "FILE_TOO_LARGE",
                        "Download exceeds file limit",
                    ));
                }
                file.write_all(&data).await?;
                hash.update(&data);
                self.transfers.update(id, |j| j.received_bytes = received);
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let sha = format!("{:x}", hash.finalize());
        let final_path = cfg.download_dir.join(format!("{id}-{filename}"));
        self.transfers.publish(
            id,
            &path,
            Artifact {
                path: final_path,
                filename,
                size: received,
                sha256: sha.clone(),
                owned: true,
                peer_session: None,
            },
        )?;
        self.log(
            "info",
            "transfer",
            "Source file ready and SHA-256 calculated",
            id,
        );
        Ok(())
    }
    pub fn transfer_status_reply(&self, p: Value) -> Result<Reply> {
        Ok(Reply::json(serde_json::to_value(
            self.transfers.get(id_param(&p)?)?,
        )?))
    }
    pub async fn cancel_transfer(&self, id: Uuid) -> Result<()> {
        self.transfers.cancel(id)?;
        self.transfers.failure(id, &cancelled());
        let upload = { self.transfers.uploads.lock().unwrap().get(&id).cloned() };
        if let Some(upload) = upload {
            let mut u = upload.lock().await;
            u.file.take();
            let cleanup = remove_file_if_present(&u.path).await;
            self.transfers.failure(id, &cancelled());
            cleanup?;
            self.transfers.uploads.lock().unwrap().remove(&id);
        }
        Ok(())
    }
    pub async fn cancel_transfer_reply(&self, p: Value) -> Result<Reply> {
        self.cancel_transfer(id_param(&p)?).await?;
        Ok(Reply::json(json!({"cancelling":true})))
    }
    pub fn start_download(self: &Arc<Self>, p: Value) -> Result<Value> {
        let target = p["target"].as_str().unwrap_or("peer");
        if target == "local" {
            return Ok(self.start_source_download(p)?.value);
        }
        if target != "peer" {
            return Err(BridgeError::invalid("target must be local or peer"));
        }
        if !self.peer.lock().unwrap().as_ref().is_some_and(|p| p.ready) {
            return Err(BridgeError::new("PEER_DISCONNECTED", "Peer is offline"));
        }
        let filename = safe_filename(p["filename"].as_str().unwrap_or("download.bin"))?;
        let permit = self
            .transfers
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new("BUSY", "At most two transfer jobs per node"))?;
        let (id, cancel) = self
            .transfers
            .new_job(filename.clone(), "receive", "peer")?;
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut remote = None;
            let result = tokio::select! {_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
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
            if let Err(e) = result {
                node.transfers.failure(id, &e);
                let _ = fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;
                if let Some(rid) = remote {
                    let _ = node
                        .peer_call("download.cancel", json!({"id":rid}), Bytes::new(), 3000)
                        .await;
                }
                node.log("error", "transfer", e.code, id);
            }
            // Source artifacts intentionally remain available until explicit deletion; never silently delete a peer's file.
        });
        Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    async fn receive_file(
        self: &Arc<Self>,
        id: Uuid,
        remote: Uuid,
        filename: String,
        total: u64,
        expected: &str,
    ) -> Result<()> {
        let cfg = self.cfg();
        let _budget = self.budget(total).await?;
        let path = cfg.download_dir.join(format!("{id}.part"));
        let mut file = File::from_std(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?,
        );
        let mut hash = Sha256::new();
        let mut offset = 0u64;
        self.transfers.update(id, |j| {
            j.status = "running".into();
            j.stage = "serial_transfer".into();
            j.received_bytes = 0;
            j.total_bytes = Some(total)
        });
        while offset < total {
            let n = (total - offset).min(cfg.chunk_bytes as u64);
            let r = self
                .peer_call(
                    "file.read",
                    json!({"id":remote,"offset":offset,"length":n}),
                    Bytes::new(),
                    60000,
                )
                .await?;
            if r.data.len() as u64 != n || r.value["offset"].as_u64() != Some(offset) {
                return Err(BridgeError::invalid("Peer returned an invalid file block"));
            }
            file.write_all(&r.data).await?;
            hash.update(&r.data);
            offset += n;
            self.transfers.update(id, |j| j.received_bytes = offset);
        }
        let actual = format!("{:x}", hash.finalize());
        if actual != expected {
            return Err(BridgeError::new(
                "HASH_MISMATCH",
                "SHA-256 verification failed; partial file is not published",
            ));
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let final_path = cfg.download_dir.join(format!("{id}-{filename}"));
        self.transfers.publish(
            id,
            &path,
            Artifact {
                path: final_path,
                filename,
                size: total,
                sha256: actual.clone(),
                owned: true,
                peer_session: None,
            },
        )?;
        self.log("info", "transfer", "File received; SHA-256 matches", id);
        Ok(())
    }
    pub async fn open_shared(&self, p: Value) -> Result<Reply> {
        self.open_shared_owned(p, None).await
    }
    pub(crate) async fn open_shared_owned(
        &self,
        p: Value,
        peer_session: Option<Uuid>,
    ) -> Result<Reply> {
        let filename = safe_filename(p["filename"].as_str().unwrap_or(""))?;
        let root = fs::canonicalize(self.cfg().shared_dir).await?;
        let path = fs::canonicalize(root.join(&filename)).await?;
        if !path.starts_with(&root) {
            return Err(BridgeError::new(
                "FORBIDDEN",
                "File is outside the shared directory",
            ));
        }
        let m = fs::metadata(&path).await?;
        if !m.is_file() || m.len() > self.cfg().max_file_bytes {
            return Err(BridgeError::invalid(
                "Not a regular file, or file is too large",
            ));
        }
        let hash = hash_file(&path).await?;
        let id = Uuid::new_v4();
        self.transfers.register(
            id,
            Artifact {
                path,
                filename: filename.clone(),
                size: m.len(),
                sha256: hash.clone(),
                owned: false,
                peer_session,
            },
        )?;
        if !self.peer_session_active(peer_session) {
            self.transfers.files.lock().unwrap().remove(&id);
            return Err(BridgeError::new(
                "PEER_DISCONNECTED",
                "Peer session closed while opening shared file",
            ));
        }
        Ok(Reply::json(
            json!({"id":id,"filename":filename,"total_bytes":m.len(),"sha256":hash}),
        ))
    }
    pub async fn read_block(&self, p: Value) -> Result<Reply> {
        let a = self.transfers.artifact(id_param(&p)?)?;
        let offset = u64_param(&p, "offset")?;
        let len = u64_param(&p, "length")?;
        if len == 0 || len > 16384 || offset > a.size || len > a.size - offset {
            return Err(BridgeError::invalid(
                "Invalid file block range (max 16 KiB)",
            ));
        }
        let mut file = File::open(a.path).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut data = vec![0; len as usize];
        file.read_exact(&mut data).await?;
        Ok(Reply {
            value: json!({"offset":offset}),
            data: Bytes::from(data),
        })
    }
    pub fn start_pull(self: &Arc<Self>, p: Value) -> Result<Value> {
        let filename = safe_filename(p["filename"].as_str().unwrap_or(""))?;
        let permit = self
            .transfers
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new("BUSY", "Transfer concurrency limit"))?;
        let (id, cancel) = self
            .transfers
            .new_job(filename.clone(), "receive", "peer")?;
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut remote = None;
            let result = tokio::select! {_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
                let opened=node.peer_call("file.open",json!({"filename":filename}),Bytes::new(),60000).await?;
                let rid=id_param(&opened.value)?;remote=Some(rid);let total=u64_param(&opened.value,"total_bytes")?;let hash=opened.value["sha256"].as_str().ok_or_else(||BridgeError::invalid("Missing SHA-256"))?;
                node.receive_file(id,rid,filename,total,hash).await
            }=>r};
            if let Some(rid) = remote {
                let _ = node
                    .peer_call("file.remove", json!({"id":rid}), Bytes::new(), 3000)
                    .await;
            }
            if let Err(e) = result {
                node.transfers.failure(id, &e);
                let _ = fs::remove_file(node.cfg().download_dir.join(format!("{id}.part"))).await;
            }
        });
        Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    pub async fn create_upload(&self, p: Value) -> Result<Reply> {
        self.create_upload_owned(p, None).await
    }
    pub(crate) async fn create_upload_owned(
        &self,
        p: Value,
        peer_session: Option<Uuid>,
    ) -> Result<Reply> {
        let filename = safe_filename(p["filename"].as_str().unwrap_or(""))?;
        let total = u64_param(&p, "total_bytes")?;
        let slot = self
            .transfers
            .upload_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new("BUSY", "At most two uploads"))?;
        let budget = self.budget(total).await?;
        let local = p["local_staging"].as_bool().unwrap_or(false);
        let (id, cancel) = self.transfers.new_job_owned(
            filename.clone(),
            if local { "staging" } else { "receive" },
            if local { "local" } else { "peer" },
            peer_session,
        )?;
        self.transfers.update(id, |j| {
            j.status = "running".into();
            j.stage = if local {
                "local_staging"
            } else {
                "serial_transfer"
            }
            .into();
            j.total_bytes = Some(total)
        });
        let path = self.cfg().download_dir.join(format!("{id}.part"));
        // 创建文件和登记上传之间不能被取消，否则没有句柄可供断线清理。
        let file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => File::from_std(file),
            Err(error) => {
                let error = BridgeError::io(error);
                self.transfers.failure(id, &error);
                return Err(error);
            }
        };
        let upload = Upload {
            file: Some(file),
            path,
            filename,
            total,
            written: 0,
            expected: p["sha256"].as_str().map(str::to_owned),
            hash: Sha256::new(),
            last: Instant::now(),
            cancel,
            _budget: budget,
            _slot: slot,
        };
        self.transfers
            .uploads
            .lock()
            .unwrap()
            .insert(id, Arc::new(AsyncMutex::new(upload)));
        if !self.peer_session_active(peer_session) {
            self.remove_artifact(json!({"id":id})).await?;
            return Err(BridgeError::new(
                "PEER_DISCONNECTED",
                "Peer session closed during upload creation",
            ));
        }
        Ok(Reply::json(json!({"id":id,"offset":0})))
    }
    pub async fn write_block(&self, p: Value, data: Bytes) -> Result<Reply> {
        let id = id_param(&p)?;
        let offset = u64_param(&p, "offset")?;
        let upload = self
            .transfers
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| BridgeError::new("NOT_FOUND", "Upload not found"))?;
        let mut u = upload.lock().await;
        if u.cancel.is_cancelled() {
            return Err(cancelled());
        }
        if offset != u.written
            || data.len() > 16384
            || data.is_empty()
            || data.len() as u64 > u.total - u.written
        {
            return Err(BridgeError::invalid("Invalid upload block size or offset"));
        }
        u.file
            .as_mut()
            .ok_or_else(|| BridgeError::invalid("Upload closed"))?
            .write_all(&data)
            .await?;
        u.hash.update(&data);
        u.written += data.len() as u64;
        u.last = Instant::now();
        self.transfers.update(id, |j| j.received_bytes = u.written);
        Ok(Reply::json(json!({"offset":u.written})))
    }
    pub async fn finish_upload(&self, p: Value) -> Result<Reply> {
        let id = id_param(&p)?;
        let upload = self
            .transfers
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| BridgeError::new("NOT_FOUND", "Upload not found"))?;
        let mut u = upload.lock().await;
        if u.file.is_none() {
            return Err(BridgeError::invalid("Upload already closed"));
        }
        let result = async {
            let actual = format!("{:x}", u.hash.clone().finalize());
            if u.cancel.is_cancelled() {
                u.file.take();
                let _ = fs::remove_file(&u.path).await;
                self.transfers.failure(id, &cancelled());
                return Err(cancelled());
            }
            if u.written != u.total || u.expected.as_ref().is_some_and(|e| e != &actual) {
                u.file.take();
                let _ = fs::remove_file(&u.path).await;
                let e = BridgeError::new("HASH_MISMATCH", "Upload size or SHA-256 mismatch");
                self.transfers.failure(id, &e);
                return Err(e);
            }
            let mut file = u
                .file
                .take()
                .ok_or_else(|| BridgeError::invalid("Upload already closed"))?;
            file.flush().await?;
            file.sync_all().await?;
            drop(file);
            let final_path = self.cfg().download_dir.join(format!("{id}-{}", u.filename));
            self.transfers.publish(
                id,
                &u.path,
                Artifact {
                    path: final_path,
                    filename: u.filename.clone(),
                    size: u.total,
                    sha256: actual.clone(),
                    owned: true,
                    peer_session: None,
                },
            )?;
            self.log(
                "info",
                "transfer",
                "Incoming upload complete; checksum verified",
                id,
            );
            Ok(Reply::json(
                json!({"id":id,"filename":u.filename,"total_bytes":u.total,"sha256":actual}),
            ))
        }
        .await;
        if let Err(error) = &result {
            u.cancel.cancel();
            u.file.take();
            let _ = fs::remove_file(&u.path).await;
            self.transfers.failure(id, error);
        }
        self.transfers.uploads.lock().unwrap().remove(&id);
        result
    }
    pub fn start_push(self: &Arc<Self>, artifact: Uuid) -> Result<Value> {
        let source = self.transfers.artifact(artifact)?;
        let permit = self
            .transfers
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new("BUSY", "Transfer concurrency limit"))?;
        let (id, cancel) = self
            .transfers
            .new_job(source.filename.clone(), "send", "peer")?;
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut remote = None;
            let result = tokio::select! {_=cancel.cancelled()=>Err(cancelled()),_=node.shutdown.cancelled()=>Err(cancelled()),r=async{
                let r=node.peer_call("file.create",json!({"filename":source.filename,"total_bytes":source.size,"sha256":source.sha256}),Bytes::new(),60000).await?;let rid=id_param(&r.value)?;remote=Some(rid);let mut file=File::open(&source.path).await?;let mut offset=0;
                node.transfers.update(id,|j|{j.status="running".into();j.stage="serial_transfer".into();j.total_bytes=Some(source.size)});
                while offset<source.size{let n=(source.size-offset).min(node.cfg().chunk_bytes as u64)as usize;let mut data=vec![0;n];file.read_exact(&mut data).await?;let r=node.peer_call("file.write",json!({"id":rid,"offset":offset}),Bytes::from(data),60000).await?;offset+=n as u64;if r.value["offset"].as_u64()!=Some(offset){return Err(BridgeError::invalid("Upload acknowledgement mismatch"))}node.transfers.update(id,|j|j.received_bytes=offset);}
                let result=node.peer_call("file.finish",json!({"id":rid}),Bytes::new(),60000).await?;if result.value["sha256"].as_str()!=Some(source.sha256.as_str()){return Err(BridgeError::new("HASH_MISMATCH","Peer checksum mismatch"))}
                node.transfers.update(id,|j|{j.status="completed".into();j.stage="completed".into();j.sha256=Some(source.sha256.clone())});Ok(())
            }=>r};
            if let Err(e) = result {
                node.transfers.failure(id, &e);
                if let Some(rid) = remote {
                    let _ = node
                        .peer_call("file.remove", json!({"id":rid}), Bytes::new(), 3000)
                        .await;
                }
            }
        });
        Ok(serde_json::to_value(self.transfers.get(id)?)?)
    }
    pub async fn remove_artifact(&self, p: Value) -> Result<Reply> {
        let id = id_param(&p)?;
        let upload = { self.transfers.uploads.lock().unwrap().get(&id).cloned() };
        if let Some(u) = upload {
            let mut u = u.lock().await;
            u.cancel.cancel();
            u.file.take();
            self.transfers.failure(id, &cancelled());
            remove_file_if_present(&u.path).await?;
            self.transfers.uploads.lock().unwrap().remove(&id);
        }
        let artifact = { self.transfers.files.lock().unwrap().get(&id).cloned() };
        if let Some(a) = artifact {
            if a.owned {
                remove_file_if_present(&a.path).await?;
            }
            self.transfers.files.lock().unwrap().remove(&id);
        }
        {
            let mut jobs = self.transfers.jobs.lock().unwrap();
            if jobs.get(&id).is_some_and(|j| {
                ["completed", "failed", "cancelled"].contains(&j.view.status.as_str())
            }) {
                jobs.remove(&id);
            }
        }
        Ok(Reply::json(json!({"removed":true})))
    }
    pub async fn reap_uploads(&self) {
        let uploads: Vec<_> = self
            .transfers
            .uploads
            .lock()
            .unwrap()
            .iter()
            .map(|(id, u)| (*id, u.clone()))
            .collect();
        for (id, upload) in uploads {
            self.abort_stale_upload(id, upload).await;
        }
    }
    /// 过期清理与 finish_upload 通过上传锁互斥，但收集列表与拿锁之间存在
    /// 窗口：finish 可能已经完成发布。拿锁后必须重查过期与任务终态，已到
    /// 终态的任务绝不被清理撤销。取消记录保留，与手动取消的语义一致。
    async fn abort_stale_upload(&self, id: Uuid, upload: Arc<AsyncMutex<Upload>>) {
        let mut u = upload.lock().await;
        let finished = self
            .transfers
            .jobs
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|j| {
                ["completed", "failed", "cancelled"].contains(&j.view.status.as_str())
            });
        let stale = u.cancel.is_cancelled() || u.last.elapsed() > Duration::from_secs(300);
        if finished || !stale {
            return;
        }
        u.cancel.cancel();
        u.file.take();
        self.transfers.failure(id, &cancelled());
        if let Err(error) = remove_file_if_present(&u.path).await {
            self.log(
                "warn",
                "transfer",
                format!("Cannot remove stale partial file: {error}"),
                id,
            );
        }
        self.transfers.uploads.lock().unwrap().remove(&id);
    }
}
async fn remove_file_if_present(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
async fn hash_file(path: &PathBuf) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0; 65536];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropping_upload_creation_does_not_leave_an_unreturned_running_job() {
        use std::{future::Future, task::Poll};
        let (_dir, node) = test_node();
        let mut create =
            Box::pin(node.create_upload(json!({"filename":"cancel-create.bin","total_bytes":0})));
        let returned = std::future::poll_fn(|cx| match create.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(Some(result.unwrap())),
            Poll::Pending if !node.transfers.list().is_empty() => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        })
        .await;
        drop(create);
        if let Some(reply) = returned {
            node.remove_artifact(json!({"id":reply.value["id"]}))
                .await
                .unwrap();
        }
        assert!(
            node.transfers.list().iter().all(|j| j.status != "running"),
            "取消 file.create 后留下未返回给调用方的活动任务"
        );
        assert_eq!(
            std::fs::read_dir(node.cfg().download_dir).unwrap().count(),
            0
        );
    }
    #[tokio::test]
    async fn stale_peer_session_cannot_create_transfer_resources() {
        let (_dir, node) = test_node();
        let old = Uuid::new_v4();
        node.config.write().unwrap().allowed_origins = vec!["https://example.test".into()];
        let local = node
            .create_upload(json!({"filename":"local.bin","total_bytes":0}))
            .await
            .unwrap();
        assert_eq!(
            node.create_upload_owned(json!({"filename":"late.bin","total_bytes":0}), Some(old))
                .await
                .unwrap_err()
                .code,
            "PEER_DISCONNECTED"
        );
        assert_eq!(
            node.start_source_download_owned(
                json!({"url":"https://example.test/file","filename":"late-source.bin"}),
                Some(old)
            )
            .unwrap_err()
            .code,
            "PEER_DISCONNECTED"
        );
        let path = node.cfg().shared_dir.join("shared.bin");
        fs::write(&path, b"shared").await.unwrap();
        assert_eq!(
            node.open_shared_owned(json!({"filename":"shared.bin"}), Some(old))
                .await
                .unwrap_err()
                .code,
            "PEER_DISCONNECTED"
        );
        assert!(node.transfers.files.lock().unwrap().is_empty());
        assert_eq!(
            node.transfers
                .get(id_param(&local.value).unwrap())
                .unwrap()
                .status,
            "running"
        );
        assert!(path.exists());
        node.create_upload(json!({"filename":"available.bin","total_bytes":0}))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn stale_upload_is_reaped_and_releases_capacity() {
        let (_dir, node) = test_node();
        let start = node
            .create_upload(json!({"filename":"stale.bin","total_bytes":4}))
            .await
            .unwrap();
        let id = id_param(&start.value).unwrap();
        let upload = node
            .transfers
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap();
        upload.lock().await.last = Instant::now() - Duration::from_secs(400);
        node.reap_uploads().await;
        assert_eq!(node.transfers.get(id).unwrap().status, "cancelled");
        assert!(!node.cfg().download_dir.join(format!("{id}.part")).exists());
        assert!(node.transfers.uploads.lock().unwrap().is_empty());
        drop(upload); // 释放测试持有的引用，让 Upload 析构返还容量。
                      // 容量随清理释放：可以立刻再开两个上传。
        for filename in ["one", "two"] {
            node.create_upload(json!({"filename":filename,"total_bytes":0}))
                .await
                .unwrap();
        }
    }
    #[tokio::test]
    async fn reaping_after_a_concurrent_finish_preserves_the_published_file() {
        let (_dir, node) = test_node();
        let start = node
            .create_upload(json!({"filename":"race.bin","total_bytes":0}))
            .await
            .unwrap();
        let id = id_param(&start.value).unwrap();
        let upload = node
            .transfers
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap();
        upload.lock().await.last = Instant::now() - Duration::from_secs(400);
        // reap 已收集这个过期上传；此刻 finish 抢先完成并发布了文件。
        node.finish_upload(json!({"id":id})).await.unwrap();
        // 过期清理随后才拿到上传锁：已完成的任务绝不能被撤销。
        node.abort_stale_upload(id, upload).await;
        assert_eq!(node.transfers.get(id).unwrap().status, "completed");
        let artifact = node.transfers.artifact(id).unwrap();
        assert!(artifact.path.exists());
    }
    fn test_node() -> (tempfile::TempDir, Arc<Node>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            download_dir: dir.path().join("downloads"),
            shared_dir: dir.path().join("shared"),
            ..crate::config::Config::default()
        };
        std::fs::create_dir_all(&cfg.download_dir).unwrap();
        std::fs::create_dir_all(&cfg.shared_dir).unwrap();
        let node = Node::new(cfg, dir.path().join("config.toml")).unwrap();
        (dir, node)
    }
    #[tokio::test]
    async fn dropped_finalization_remains_cancellable_and_reapable() {
        use std::future::Future;
        let (_dir, node) = test_node();
        let start = node
            .create_upload(json!({"filename":"empty.bin","total_bytes":0}))
            .await
            .unwrap();
        let id = id_param(&start.value).unwrap();
        let upload = node
            .transfers
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap();
        let locked = upload.lock().await;
        let mut finish = Box::pin(node.finish_upload(json!({"id":id})));
        std::future::poll_fn(|cx| {
            assert!(finish.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(finish);
        drop(locked);
        drop(upload);
        node.cancel_transfer(id).await.unwrap();
        assert_eq!(node.transfers.get(id).unwrap().status, "cancelled");
        assert!(!node.cfg().download_dir.join(format!("{id}.part")).exists());
        for filename in ["one", "two"] {
            node.create_upload(json!({"filename":filename,"total_bytes":0}))
                .await
                .unwrap();
        }
    }
    #[tokio::test]
    async fn cancelled_pull_releases_remote_shared_handle_and_preserves_original() {
        let (_ad, a) = test_node();
        let (_bd, b) = test_node();
        let original = b.cfg().shared_dir.join("large.bin");
        fs::write(&original, vec![42_u8; 2 * 1024 * 1024])
            .await
            .unwrap();
        let (left, right) = tokio::io::duplex(1024);
        let aa = a.clone();
        let bb = b.clone();
        let ha = tokio::spawn(async move { crate::transport::session(aa, left).await });
        let hb = tokio::spawn(async move { crate::transport::session(bb, right).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !a.peer.lock().unwrap().as_ref().is_some_and(|p| p.ready) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let started = a.start_pull(json!({"filename":"large.bin"})).unwrap();
        let id = id_param(&started).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while a.transfers.get(id).unwrap().stage != "serial_transfer" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        a.cancel_transfer(id).await.unwrap();
        let released = tokio::time::timeout(Duration::from_secs(1), async {
            while !b.transfers.files.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        ha.abort();
        hb.abort();
        let _ = ha.await;
        let _ = hb.await;
        assert!(
            released.is_ok(),
            "cancelled pull leaked the remote file handle"
        );
        assert!(original.exists());
        assert_eq!(a.transfers.get(id).unwrap().status, "cancelled");
    }
    #[tokio::test]
    async fn upload_enforces_offsets_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            download_dir: dir.path().into(),
            shared_dir: dir.path().into(),
            ..crate::config::Config::default()
        };
        let node = Node::new(cfg, dir.path().join("config.toml")).unwrap();
        let digest = format!("{:x}", Sha256::digest(b"hello"));
        let start = node
            .create_upload(json!({"filename":"test.bin","total_bytes":5,"sha256":digest}))
            .await
            .unwrap();
        let id = &start.value["id"];
        assert!(node
            .write_block(json!({"id":id,"offset":1}), Bytes::from_static(b"hello"))
            .await
            .is_err());
        node.write_block(json!({"id":id,"offset":0}), Bytes::from_static(b"hello"))
            .await
            .unwrap();
        let result = node.finish_upload(json!({"id":id})).await.unwrap();
        assert_eq!(result.value["sha256"], digest);
    }
    #[tokio::test]
    async fn empty_file_hash_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            download_dir: dir.path().into(),
            ..crate::config::Config::default()
        };
        let node = Node::new(cfg, dir.path().join("config.toml")).unwrap();
        let start=node.create_upload(json!({"filename":"empty","total_bytes":0,"sha256":format!("{:x}",Sha256::digest([]))})).await.unwrap();
        assert!(node
            .finish_upload(json!({"id":start.value["id"]}))
            .await
            .is_ok());
    }
}
