export interface Policy {origins:string[];methods:string[];maxInlineBytes:number}
export interface FetchRequest {url:string;method?:string;headers?:Record<string,string>;body?:string|null}
export interface FetchResult {status:number;headers:Record<string,string>;body?:string;body_base64?:string;body_encoding:string}
const reserved=/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
export function safeFilename(name:string):string {
 if(!name || name.length>120 || /[<>:"/\\|?*\x00-\x1f]/.test(name) || /[. ]$/.test(name) || name==='.' || name==='..' || reserved.test(name)) throw Error('INVALID_FILENAME');
 return name;
}
export function validateRequest(req:FetchRequest,policy:Policy):Required<Pick<FetchRequest,'url'|'method'|'headers'>> & Pick<FetchRequest,'body'> {
 const u=new URL(req.url);const method=(req.method??'GET').toUpperCase();
 if(!['http:','https:'].includes(u.protocol) || u.username || u.password || !policy.origins.includes(u.origin)) throw Error('ORIGIN_NOT_ALLOWED');
 if(!policy.methods.includes(method)) throw Error('METHOD_NOT_ALLOWED');
 const headers=req.headers??{};
 if(Object.keys(headers).length>32) throw Error('TOO_MANY_HEADERS');
 for(const [key,value] of Object.entries(headers)) {
  if(/^(cookie|host|origin|referer|content-length|connection|set-cookie|transfer-encoding)$/i.test(key) || /^(sec-|proxy-)/i.test(key)) throw Error('FORBIDDEN_HEADER');
  if(/[\r\n]/.test(key+value) || key.length>128 || value.length>4096) throw Error('INVALID_HEADER');
 }
 if(req.body && new TextEncoder().encode(req.body).length>8192) throw Error('REQUEST_TOO_LARGE');
 if((method==='GET'||method==='HEAD')&&req.body) throw Error('BODY_NOT_ALLOWED');
 return {url:u.href,method,headers,body:req.body};
}
export async function readLimited(response:Response,max:number,signal?:AbortSignal):Promise<Uint8Array> {
 const declared=response.headers.get('content-length');
 if(declared && /^\d+$/.test(declared) && Number(declared)>max) {await response.body?.cancel();throw Error('RESPONSE_TOO_LARGE')}
 if(!response.body) return new Uint8Array();
 const reader=response.body.getReader();const chunks:Uint8Array[]=[];let size=0;
 try {
  for(;;){signal?.throwIfAborted();const {done,value}=await reader.read();signal?.throwIfAborted();if(done)break;size+=value.byteLength;if(size>max)throw Error('RESPONSE_TOO_LARGE');chunks.push(value)}
 } catch(e) {await reader.cancel().catch(()=>{});throw e} finally {reader.releaseLock()}
 const result=new Uint8Array(size);let at=0;for(const b of chunks){result.set(b,at);at+=b.length}return result;
}
export async function authorizedFetch(req:FetchRequest,policy:Policy,signal:AbortSignal,deps:{hasPermission:(url:string)=>Promise<boolean>;fetch:typeof fetch}):Promise<Response> {
 const validated=validateRequest(req,policy);signal.throwIfAborted();
 if(!await deps.hasPermission(validated.url)) throw Error('HOST_PERMISSION_REQUIRED');
 // Permissions resolve asynchronously: cancellation must be checked again before any network side effect.
 signal.throwIfAborted();
 return deps.fetch(validated.url,{method:validated.method,headers:validated.headers,body:validated.body??undefined,credentials:'include',redirect:'error',cache:'no-store',signal});
}
export async function executeFetch(req:FetchRequest,policy:Policy,signal:AbortSignal,deps:{hasPermission:(url:string)=>Promise<boolean>;fetch:typeof fetch}):Promise<FetchResult> {
 const res=await authorizedFetch(req,policy,signal,deps);const bytes=await readLimited(res,policy.maxInlineBytes,signal);
 const headers=Object.fromEntries([...res.headers].filter(([k])=>k.toLowerCase()!=='set-cookie'));
 try {return {status:res.status,headers,body:new TextDecoder('utf-8',{fatal:true}).decode(bytes),body_encoding:'utf8'}}
 catch {return {status:res.status,headers,body_base64:btoa(String.fromCharCode(...bytes)),body_encoding:'base64'}}
}
export function binaryMessage(meta:Record<string,unknown>,data:Uint8Array):ArrayBuffer {
 const json=new TextEncoder().encode(JSON.stringify(meta));
 if(json.length>4096||data.length>65536) throw Error('FRAME_TOO_LARGE');
 const result=new Uint8Array(4+json.length+data.length);new DataView(result.buffer).setUint32(0,json.length,false);result.set(json,4);result.set(data,4+json.length);return result.buffer;
}
export function parseBinaryMessage(frame:ArrayBuffer):{meta:Record<string,unknown>;data:Uint8Array} {
 if(frame.byteLength<4||frame.byteLength>69636) throw Error('INVALID_FRAME');
 const len=new DataView(frame).getUint32(0,false);if(len>4096||len+4>frame.byteLength)throw Error('INVALID_FRAME');
 const meta=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(new Uint8Array(frame,4,len)));
 if(!meta||typeof meta!=='object'||Array.isArray(meta))throw Error('INVALID_METADATA');
 const data=new Uint8Array(frame,4+len);if(data.length>65536)throw Error('FRAME_TOO_LARGE');return {meta,data};
}
