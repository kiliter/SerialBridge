import test from 'node:test';
import assert from 'node:assert/strict';
import {validateRequest, readLimited, executeFetch, safeFilename, binaryMessage, parseBinaryMessage} from '../browser-extension/dist/executor.js';
const policy = {origins:['https://example.test'],methods:['GET','HEAD'],maxInlineBytes:16384};
test('allows only the exact origin, not a suffix match',()=>{
 assert.equal(validateRequest({url:'https://example.test/api',method:'GET'},policy).method,'GET');
 for (const url of ['https://example.test.evil/a','http://example.test/a','https://example.test:444/a','file:///etc/passwd','https://u:p@example.test/a']) {
  assert.throws(()=>validateRequest({url,method:'GET'},policy));
 }
});
test('method and sensitive headers are denied',()=>{
 assert.throws(()=>validateRequest({url:'https://example.test',method:'POST'},policy));
 for(const key of ['Cookie','HOST','Origin','Referer','Content-Length','Proxy-Authorization','Sec-Fetch-Site'])
  assert.throws(()=>validateRequest({url:'https://example.test',headers:{[key]:'bad'}},policy));
});
test('file names cannot escape a directory or use Windows reserved names',()=>{
 assert.equal(safeFilename('report.zip'),'report.zip');
 for(const name of ['../x','a/b','a\\b','..','CON','con.txt','LPT1.zip','a:foo','foo.','foo ','\0','']) assert.throws(()=>safeFilename(name));
});
test('limited streaming reads reject a response without content-length above the cap',async()=>{
 const stream=new ReadableStream({start(c){c.enqueue(new Uint8Array(8));c.enqueue(new Uint8Array(8));c.close()}});
 await assert.rejects(readLimited(new Response(stream),10),/TOO_LARGE/);
});
test('empty and binary bodies remain byte-exact',async()=>{
 assert.deepEqual([...await readLimited(new Response(new Uint8Array([0,255,23])),10)],[0,255,23]);
 assert.equal((await readLimited(new Response(null),10)).byteLength,0);
});
test('HEAD 的资源长度不计入空响应体大小',async()=>{
 const result=await executeFetch({url:'https://example.test/file',method:'HEAD'},policy,new AbortController().signal,{
  hasPermission:async()=>true,
  fetch:async()=>new Response(null,{headers:{'content-length':'1048576'}})
 });
 assert.equal(result.body,'');
});
test('permission check returning after disconnect never calls fetch',async()=>{
 let grant; const permission=new Promise(r=>grant=r); const controller=new AbortController(); let calls=0;
 const run=executeFetch({url:'https://example.test',method:'GET'},policy,controller.signal,{hasPermission:()=>permission,fetch:async()=>{calls++;return new Response('bad')}});
 controller.abort(); grant(true); await assert.rejects(run); assert.equal(calls,0);
});
test('browser fetch uses credentials and rejects redirects',async()=>{
 let options;
 const result=await executeFetch({url:'https://example.test/api'},policy,new AbortController().signal,{hasPermission:async()=>true,fetch:async(_,opts)=>{options=opts;return new Response('ok',{status:201,headers:{'content-type':'text/plain'}})}});
 assert.equal(options.credentials,'include'); assert.equal(options.redirect,'error'); assert.equal(result.status,201); assert.equal(result.body,'ok');
});
test('missing browser permission prevents fetch',async()=>{
 let calls=0;
 await assert.rejects(executeFetch({url:'https://example.test'},policy,new AbortController().signal,{hasPermission:async()=>false,fetch:async()=>{calls++;return new Response('bad')}}),/PERMISSION/);
 assert.equal(calls,0);
});
test('binary WebSocket envelope roundtrips and enforces bounds',()=>{
 const frame=binaryMessage({type:'chunk',id:'x',offset:0},new Uint8Array([0,1,255]));
 const p=parseBinaryMessage(frame);assert.equal(p.meta.id,'x');assert.deepEqual([...p.data],[0,1,255]);
 assert.throws(()=>parseBinaryMessage(new ArrayBuffer(2)));
 assert.throws(()=>parseBinaryMessage(new Uint8Array([255,255,255,255]).buffer));
 assert.throws(()=>binaryMessage({},new Uint8Array(65537)));
});
