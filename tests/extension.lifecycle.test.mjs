/** Runs the real service-worker module with Chrome/WebSocket test doubles, no browser networking. */
import test from 'node:test';import assert from 'node:assert/strict';
import {parseBinaryMessage} from '../browser-extension/dist/executor.js';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function waitFor(fn){for(let n=0;n<200;n++){const value=fn();if(value)return value;await sleep(5)}throw Error('fixture timeout')}
test('service-worker pairing, bounded fetch, binary download and disconnect using test doubles',async()=>{
 const previousChrome=globalThis.chrome, previousWebSocket=globalThis.WebSocket,previousFetch=globalThis.fetch;
 const storage={enabled:true,port:9527,pairCode:'ABCDEF123456'};let listener;const sockets=[];let networkCalls=0;
 class MockSocket{
  static OPEN=1;static CLOSED=3;readyState=0;sent=[];
  constructor(url){this.url=url;sockets.push(this);queueMicrotask(()=>{this.readyState=1;this.onopen?.()})}
  send(data){this.sent.push(data);if(typeof data!=='string'){const{meta,data:bytes}=parseBinaryMessage(data);queueMicrotask(()=>this.receive({type:'chunk.ack',id:meta.id,offset:Number(meta.offset)+bytes.length}))}}
  receive(value){this.onmessage?.({data:JSON.stringify(value)})}
  close(){this.readyState=3;queueMicrotask(()=>this.onclose?.())}
 }
 globalThis.WebSocket=MockSocket;
 globalThis.chrome={runtime:{id:'a'.repeat(32),getURL:p=>`chrome-extension://${'a'.repeat(32)}/${p}`,onMessage:{addListener:f=>listener=f},onInstalled:{addListener:()=>{}},onStartup:{addListener:()=>{}}},storage:{local:{get:async keys=>Object.fromEntries(keys.map(k=>[k,storage[k]])),set:async values=>Object.assign(storage,values),remove:async keys=>{for(const k of Array.isArray(keys)?keys:[keys])delete storage[k]},setAccessLevel:async()=>{}}},permissions:{contains:async()=>true},alarms:{create:async()=>{},onAlarm:{addListener:()=>{}}}};
 const expected=new Uint8Array(140000);for(let n=0;n<expected.length;n++)expected[n]=n%251;
 globalThis.fetch=async url=>{networkCalls++;return String(url).endsWith('/file')?new Response(expected,{headers:{'content-length':String(expected.length)}}):new Response('fixture-api',{headers:{'content-type':'text/plain'}})};
 const message=(data)=>new Promise(resolve=>listener(data,{id:'a'.repeat(32),url:`chrome-extension://${'a'.repeat(32)}/popup.html`},resolve));
 try{
  await import('../browser-extension/dist/background.js');const ws=await waitFor(()=>sockets[0]?.sent.length?sockets[0]:null);
  assert.deepEqual(JSON.parse(ws.sent[0]),{type:'pair',code:'ABCDEF123456'});assert.equal(ws.url,'ws://127.0.0.1:9527/v1/extension/ws');
  ws.receive({type:'ready',token:'TEST-ONLY-EXTENSION-TOKEN',node:'fixture-node',session:'fixture'});
  await waitFor(()=>storage.token);await waitFor(()=>!storage.pairCode);
  const policy={origins:['https://example.test'],methods:['GET'],maxInlineBytes:16384,maxFileBytes:200000};
  ws.receive({type:'task.execute',id:'fetch-task',method:'browser.fetch',params:{url:'https://example.test/api'},policy});
  const result=await waitFor(()=>ws.sent.filter(x=>typeof x==='string').map(JSON.parse).find(m=>m.id==='fetch-task'&&m.type==='task.result'));
  assert.equal(result.result.body,'fixture-api');
  ws.receive({type:'task.execute',id:'denied-task',method:'browser.fetch',params:{url:'https://evil.test/api'},policy});
  await waitFor(()=>ws.sent.filter(x=>typeof x==='string').map(JSON.parse).find(m=>m.id==='denied-task'&&m.type==='task.error'));assert.equal(networkCalls,1);
  ws.receive({type:'task.execute',id:'file-task',method:'browser.download',params:{url:'https://example.test/file'},policy});
  const end=await waitFor(()=>ws.sent.filter(x=>typeof x==='string').map(JSON.parse).find(m=>m.id==='file-task'&&m.type==='download.end'));
  assert.equal(end.total_bytes,expected.length);
  const blocks=ws.sent.filter(x=>typeof x!=='string').map(parseBinaryMessage);assert.equal(blocks.length,3);assert.ok(blocks.every(b=>b.data.length<=65536));assert.deepEqual(Buffer.concat(blocks.map(b=>Buffer.from(b.data))),Buffer.from(expected));
  assert.equal((await message({type:'status'})).state,'connected');
  await message({type:'disconnect'});await sleep(10);assert.equal((await message({type:'status'})).state,'disconnected');assert.equal(storage.enabled,false);
 }finally{
  if(listener)await message({type:'disconnect'}).catch(()=>{});globalThis.chrome=previousChrome;globalThis.WebSocket=previousWebSocket;globalThis.fetch=previousFetch;
 }
});
