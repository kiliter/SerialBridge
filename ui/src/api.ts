import type {Snapshot,Transfer,Settings} from './types.js';
export const demo=new URLSearchParams(location.search).get('demo')==='1';
export const native=Boolean(window.__TAURI__);
function stored(key:string):string|null {try{return sessionStorage.getItem(key)}catch{return null}}
let token=stored('serialbridge.token')??'';
let port=Number(stored('serialbridge.port')??9527);
export function configure(p:number,t:string):void {if(!Number.isInteger(p)||p<1||p>65535)throw Error('无效端口');port=p;token=t;try{sessionStorage.setItem('serialbridge.port',String(p));sessionStorage.setItem('serialbridge.token',t)}catch{/* Restricted webviews can still use in-memory credentials. */}}
export function hasCredentials():boolean {return demo||native||Boolean(token)}
export async function api<T>(method:string,path:string,body?:unknown):Promise<T>{
 if(demo)return demoApi(method,path,body)as Promise<T>;
 if(native)return window.__TAURI__!.core.invoke<T>('api',{method,path,body:body??null});
 const response=await fetch(`http://127.0.0.1:${port}${path}`,{method,headers:{Authorization:`Bearer ${token}`,...(body!==undefined?{'Content-Type':'application/json'}:{})},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(125000)});
 const data=await response.json();if(!response.ok)throw Error(data.error?.message??`HTTP ${response.status}`);return data as T;
}
export async function saveContent(id:string,filename:string):Promise<string|void>{
 if(demo)return '演示模式不生成真实下载文件';
 if(native){const r=await window.__TAURI__!.core.invoke<{path:string}>('file_location',{id});return `文件已保存在本机：\n${r.path}`}
 const res=await fetch(`http://127.0.0.1:${port}/v1/downloads/${encodeURIComponent(id)}/content`,{headers:{Authorization:`Bearer ${token}`}});if(!res.ok)throw Error(`下载失败：HTTP ${res.status}`);
 const url=URL.createObjectURL(await res.blob());const a=document.createElement('a');a.href=url;a.download=filename;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
}
export async function pushFile(file:File,onProgress:(n:number)=>void):Promise<Transfer>{
 if(demo)return await demoApi('POST','/v1/downloads',{filename:file.name})as Transfer;
 if(native){
  const invoke=window.__TAURI__!.core.invoke;let id='';try{
   const start=await invoke<{id:string}>('file_stage',{op:'create',params:{filename:file.name,total_bytes:file.size},data:[]});id=start.id;
   for(let offset=0;offset<file.size;offset+=16384){const data=new Uint8Array(await file.slice(offset,offset+16384).arrayBuffer());await invoke('file_stage',{op:'write',params:{id,offset},data:Array.from(data)});onProgress(Math.min(offset+data.length,file.size));}
   return await invoke<Transfer>('file_stage',{op:'finish',params:{id},data:[]});
  }catch(e){if(id)await invoke('file_stage',{op:'cancel',params:{id},data:[]}).catch(()=>{});throw e}
 }
 const res=await fetch(`http://127.0.0.1:${port}/v1/files/push/${encodeURIComponent(file.name)}`,{method:'PUT',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/octet-stream'},body:file});
 const data=await res.json();if(!res.ok)throw Error(data.error?.message??'上传失败');return data as Transfer;
}
// This adapter is opt-in only (?demo=1). It is never selected on connection failure.
let demoOnline=true;
const demoSettings:Settings={node_name:'Node A',serial_port:'/dev/cu.usbserial-DEMO-A',baud_rate:921600,http_port:9527,allowed_origins:['https://example.com'],allowed_methods:['GET','HEAD'],auto_reconnect:true,download_dir:'~/SerialBridge/downloads',shared_dir:'~/SerialBridge/shared',max_file_bytes:104857600,max_spool_bytes:1073741824,chunk_bytes:8192};
const demoTransfers:Transfer[]=[{id:'demo-complete',filename:'interface-report.json',direction:'receive',target:'peer',status:'completed',stage:'completed',received_bytes:48128,total_bytes:48128,sha256:'demo-not-a-real-checksum',created_at:Date.now()-240000},{id:'demo-progress',filename:'reference-package.zip',direction:'receive',target:'peer',status:'running',stage:'serial_transfer',received_bytes:786432,total_bytes:2097152,created_at:Date.now()-90000}];
async function demoApi(method:string,path:string,body?:unknown):Promise<unknown>{
 const p=body as Record<string,unknown>|undefined;
 if(path==='/v1/health')return {version:'0.1.0-alpha.1',node:{name:demoSettings.node_name,os:'macos'},http:{port:9527},serial:{port:demoSettings.serial_port,baud_rate:demoSettings.baud_rate},peer:{connected:demoOnline,name:'Node B',os:'windows',browser:demoOnline},browser:{connected:true,origin:'chrome-extension://demo'},pending_requests:0}satisfies Snapshot;
 if(path==='/v1/settings'){if(method==='PUT'){Object.assign(demoSettings,p);return {settings:demoSettings,restart_required:false}}return demoSettings}
 if(path==='/v1/ports')return {ports:[{name:'/dev/cu.usbserial-DEMO-A',product:'FT232RL · 演示串口'}]};
 if(path==='/v1/logs')return {logs:[{time:Date.now()-3100,level:'info',component:'serial',message:'[演示] Peer handshake complete',id:'demo-01'},{time:Date.now()-2200,level:'info',component:'browser',message:'[演示] Local extension authenticated',id:'demo-02'},{time:Date.now()-1100,level:'warn',component:'rpc',message:'[演示] ORIGIN_NOT_ALLOWED — 请求被策略拒绝',id:'demo-03'},{time:Date.now()-200,level:'info',component:'transfer',message:'[演示] File received; SHA-256 matches',id:'demo-04'}]};
 if(path==='/v1/downloads'&&method==='GET')return {transfers:demoTransfers};
 if((path==='/v1/downloads'||path==='/v1/files/pull')&&method==='POST'){const t:Transfer={id:`demo-${Date.now()}`,filename:String(p?.filename??'download.bin'),direction:'receive',target:'peer',status:'queued',stage:'queued',received_bytes:0,total_bytes:null,created_at:Date.now()};demoTransfers.unshift(t);return t}
 if(path.startsWith('/v1/downloads/')&&method==='DELETE'){const id=path.split('/')[3];const t=demoTransfers.find(t=>t.id===id);if(t){if(['completed','failed','cancelled'].includes(t.status))demoTransfers.splice(demoTransfers.indexOf(t),1);else t.status='cancelled'}return {cancelling:true}}
 if(path==='/v1/extension/pairing')return method==='DELETE'?{revoked:true}:{code:'DEMO00000000',expires_in_seconds:180};
 if(path==='/v1/serial/reconnect'){demoOnline=!demoOnline;return {reconnecting:true}}
 if(path==='/v1/rpc'){if(!demoOnline&&p?.target!=='local')throw Error('演示：对端未连接');return {result:{name:p?.target==='local'?'Node A':'Node B',protocol:1,demo:true}}}
 throw Error('演示模式未提供这个操作');
}
