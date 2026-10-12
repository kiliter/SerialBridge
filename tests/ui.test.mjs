import test from 'node:test';
import assert from 'node:assert/strict';
import {deriveStatus, filterLogs, progress, escapeHtml} from '../ui/dist/assets/model.js';
test('a disconnected peer never leaves remote browser online',()=>{
 const s=deriveStatus({peer:{connected:false,browser:true},browser:{connected:true}});
 assert.equal(s.remoteBrowser,false);assert.equal(s.localBrowser,true);assert.equal(s.peer,false);
});
test('missing state is unknown/offline, never optimistic online',()=>{
 const s=deriveStatus(null); assert.equal(s.core,false); assert.equal(s.peer,false);assert.equal(s.remoteBrowser,false);
});
test('log filters combine query with level and component',()=>{
 const logs=[{level:'error',component:'browser',message:'offline',id:'r1'},{level:'info',component:'serial',message:'online',id:'r2'}];
 assert.equal(filterLogs(logs,{level:'error',component:'browser',query:'OFFLINE'}).length,1);
 assert.equal(filterLogs(logs,{level:'error',component:'serial',query:''}).length,0);
});
test('unknown totals do not pretend to be 100 percent',()=>{
 assert.equal(progress(50,null),null);assert.equal(progress(0,0),null);assert.equal(progress(20,100),20);assert.equal(progress(101,100),100);
});
test('untrusted markup is escaped',()=>assert.equal(escapeHtml('<img onerror="x">'), '&lt;img onerror=&quot;x&quot;&gt;'));
