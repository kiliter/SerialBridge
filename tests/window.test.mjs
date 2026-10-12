import test from 'node:test';import assert from 'node:assert/strict';import {AckWindow} from '../browser-extension/dist/window.js';
test('only an exact matching acknowledgement advances the window',async()=>{const w=new AckWindow();const p=w.wait('a',64,new AbortController().signal);assert.equal(w.ack('b',64),false);assert.equal(w.ack('a',64),true);await p;});
test('out-of-order offset is rejected',async()=>{const w=new AckWindow();const p=w.wait('a',64,new AbortController().signal);assert.equal(w.ack('a',32),false);await assert.rejects(p,/MISMATCH/);});
test('disconnect rejects waiting chunks',async()=>{const w=new AckWindow();const p=w.wait('a',64,new AbortController().signal);w.reset();await assert.rejects(p,/DISCONNECTED/);});
test('cancellation releases the pending window',async()=>{const w=new AckWindow();const c=new AbortController();const p=w.wait('a',64,c.signal);c.abort();await assert.rejects(p,/CANCELLED/);const next=w.wait('a',64,new AbortController().signal);w.ack('a',64);await next;});
test('unacknowledged chunk has a finite deadline',async()=>{const w=new AckWindow();await assert.rejects(w.wait('a',1,new AbortController().signal,5),/ACK_TIMEOUT/);});
