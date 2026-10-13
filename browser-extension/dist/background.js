import { authorizedFetch, binaryMessage, executeFetch } from './executor.js';
import { AckWindow } from './window.js';
let status = { state: 'disconnected', message: '尚未连接本机 SerialBridge', port: 9527 };
let socket = null;
let connecting = false;
let enabled = false;
let authenticated = false;
let generation = 0;
let retry = 1000;
let reconnectTimer;
let heartbeat;
const active = new Map();
const acks = new AckWindow();
function setStatus(state, message) { status = { ...status, state, message }; }
function send(ws, data) { if (ws !== socket || ws.readyState !== WebSocket.OPEN || !authenticated)
    throw Error('BROWSER_DISCONNECTED'); ws.send(JSON.stringify(data)); }
function cancelAll() { for (const c of active.values())
    c.abort(); active.clear(); acks.reset(); }
function schedule() {
    if (!enabled)
        return;
    clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => { void connect(); }, retry);
    retry = Math.min(30000, retry * 2);
}
async function connect() {
    if (connecting || socket)
        return;
    connecting = true;
    const epoch = ++generation;
    try {
        const saved = await chrome.storage.local.get(['enabled', 'port', 'token', 'pairCode']);
        if (epoch !== generation)
            return;
        enabled = saved.enabled === true;
        if (!enabled) {
            setStatus('disconnected', '连接已暂停');
            return;
        }
        const port = Number(saved.port ?? 9527);
        if (!Number.isInteger(port) || port < 1 || port > 65535)
            throw Error('INVALID_LOCAL_PORT');
        status.port = port;
        if (!saved.token && !saved.pairCode) {
            setStatus('pairing', '请在桌面设置中生成配对码');
            return;
        }
        const ws = new WebSocket(`ws://127.0.0.1:${port}/v1/extension/ws`);
        socket = ws;
        ws.binaryType = 'arraybuffer';
        authenticated = false;
        setStatus('connecting', '正在连接本机服务…');
        ws.onopen = () => { if (epoch !== generation)
            return; ws.send(JSON.stringify(saved.pairCode ? { type: 'pair', code: saved.pairCode } : { type: 'auth', token: saved.token })); };
        ws.onmessage = event => { void handleMessage(ws, epoch, event.data).catch(() => { if (ws === socket) {
            setStatus('error', '本机服务消息异常');
            ws.close();
        } }); };
        ws.onerror = () => { if (ws === socket)
            setStatus('error', '连接失败：检查本机服务、端口和配对'); };
        ws.onclose = () => { if (ws !== socket)
            return; socket = null; authenticated = false; clearInterval(heartbeat); cancelAll(); if (status.state !== 'error')
            setStatus('disconnected', '本机服务已断开'); schedule(); };
    }
    catch {
        if (epoch === generation) {
            setStatus('error', '无法读取配置或创建本地连接');
            schedule();
        }
    }
    finally {
        connecting = false;
        // 旧配置读取被重启操作替代后，立即读取新配置，不能遗失这次连接请求。
        if (epoch !== generation && enabled)
            void connect();
    }
}
function policyFrom(v) {
    if (!v || typeof v !== 'object')
        throw Error('MISSING_POLICY');
    const p = v;
    if (!Array.isArray(p.origins) || !p.origins.every(x => typeof x === 'string') || !Array.isArray(p.methods) || !p.methods.every(x => typeof x === 'string'))
        throw Error('INVALID_POLICY');
    return { origins: p.origins, methods: p.methods, maxInlineBytes: Math.min(16384, Number(p.maxInlineBytes) || 16384), maxFileBytes: Math.min(10 * 1024 ** 3, Number(p.maxFileBytes) || 100 * 1024 ** 2) };
}
async function handleMessage(ws, epoch, data) {
    if (epoch !== generation || ws !== socket)
        return;
    if (typeof data !== 'string' || data.length > 131072)
        throw Error('INVALID_CONTROL_MESSAGE');
    const m = JSON.parse(data);
    if (m.type === 'ready') {
        authenticated = true;
        retry = 1000;
        if (typeof m.token === 'string')
            await chrome.storage.local.set({ token: m.token });
        if (epoch !== generation || ws !== socket)
            return;
        await chrome.storage.local.remove('pairCode');
        if (epoch !== generation || ws !== socket)
            return;
        status.node = String(m.node ?? 'SerialBridge');
        setStatus('connected', '已连接本机 SerialBridge');
        clearInterval(heartbeat);
        heartbeat = setInterval(() => { if (ws === socket && authenticated && ws.readyState === WebSocket.OPEN)
            ws.send(JSON.stringify({ type: 'ping' })); }, 20000);
        return;
    }
    if (m.type === 'error') {
        authenticated = false;
        const e = m.error;
        setStatus('error', e?.message ?? '配对或认证失败');
        // Invalid pairing codes are not retried forever. Keep a stored token so a transient socket error is recoverable.
        await chrome.storage.local.remove('pairCode');
        ws.close();
        return;
    }
    if (!authenticated)
        return;
    if (m.type === 'ping') {
        send(ws, { type: 'pong' });
        return;
    }
    if (m.type === 'pong')
        return;
    const id = typeof m.id === 'string' ? m.id : '';
    if (m.type === 'task.cancel') {
        active.get(id)?.abort();
        acks.reject(id, Error('CANCELLED'));
        return;
    }
    if (m.type === 'chunk.ack') {
        acks.ack(id, Number(m.offset));
        return;
    }
    if (m.type !== 'task.execute' || !id)
        return;
    if (active.has(id) || active.size >= 4) {
        send(ws, { type: 'task.error', id, message: 'BROWSER_BUSY' });
        return;
    }
    const controller = new AbortController();
    active.set(id, controller);
    const deadline = setTimeout(() => controller.abort(Error('TASK_TIMEOUT')), 300000);
    void (async () => {
        const policy = policyFrom(m.policy);
        const req = m.params;
        const deps = { hasPermission: (url) => chrome.permissions.contains({ origins: [`${new URL(url).origin}/*`] }), fetch: globalThis.fetch.bind(globalThis) };
        if (m.method === 'browser.fetch') {
            const result = await executeFetch(req, policy, controller.signal, deps);
            controller.signal.throwIfAborted();
            send(ws, { type: 'task.result', id, result });
        }
        else if (m.method === 'browser.download') {
            const res = await authorizedFetch(req, policy, controller.signal, deps);
            if (!res.ok) {
                await res.body?.cancel();
                throw Error(`HTTP_${res.status}`);
            }
            const header = res.headers.get('content-length');
            const compressed = Boolean(res.headers.get('content-encoding'));
            const total = !res.body ? 0 : !compressed && header && /^\d+$/.test(header) ? Number(header) : null;
            if (total !== null && total > policy.maxFileBytes) {
                await res.body?.cancel();
                throw Error('FILE_TOO_LARGE');
            }
            send(ws, { type: 'download.begin', id, total_bytes: total });
            let offset = 0;
            if (res.body) {
                const reader = res.body.getReader();
                try {
                    for (;;) {
                        controller.signal.throwIfAborted();
                        const { done, value } = await reader.read();
                        if (done)
                            break;
                        for (let i = 0; i < value.length; i += 65536) {
                            controller.signal.throwIfAborted();
                            const chunk = value.subarray(i, i + 65536);
                            if (offset + chunk.length > policy.maxFileBytes)
                                throw Error('FILE_TOO_LARGE');
                            const wait = acks.wait(id, offset + chunk.length, controller.signal);
                            try {
                                if (ws !== socket || !authenticated || ws.readyState !== WebSocket.OPEN)
                                    throw Error('BROWSER_DISCONNECTED');
                                ws.send(binaryMessage({ type: 'chunk', id, offset }, chunk));
                            }
                            catch {
                                acks.reject(id, Error('BROWSER_DISCONNECTED'));
                            }
                            await wait;
                            offset += chunk.length;
                        }
                    }
                }
                catch (e) {
                    await reader.cancel().catch(() => { });
                    throw e;
                }
                finally {
                    reader.releaseLock();
                }
            }
            send(ws, { type: 'download.end', id, total_bytes: offset });
        }
        else
            throw Error('METHOD_NOT_FOUND');
    })().catch(error => {
        if (ws === socket && authenticated && ws.readyState === WebSocket.OPEN) {
            send(ws, { type: 'task.error', id, message: error instanceof Error ? error.message : 'BROWSER_ERROR' });
        }
    }).finally(() => { clearTimeout(deadline); if (active.get(id) === controller)
        active.delete(id); acks.reject(id, Error('TASK_ENDED')); });
}
async function dispatch(message) {
    const m = message;
    if (m.type === 'status')
        return { ...status, activeTasks: active.size };
    if (m.type === 'configure') {
        const port = Number(m.port);
        const code = String(m.code ?? '').trim().toUpperCase();
        if (!Number.isInteger(port) || port < 1 || port > 65535 || code && !/^[A-F0-9]{12}$/.test(code))
            throw Error('端口或配对码格式错误');
        const prev = await chrome.storage.local.get(['port']);
        if (Number(prev.port ?? 9527) !== port)
            await chrome.storage.local.remove('token');
        await chrome.storage.local.set({ port, enabled: true, ...(code ? { pairCode: code } : {}) });
        enabled = true;
        restart();
        return { ok: true };
    }
    if (m.type === 'disconnect') {
        enabled = false;
        generation++;
        clearTimeout(reconnectTimer);
        clearInterval(heartbeat);
        cancelAll();
        const old = socket;
        socket = null;
        authenticated = false;
        old?.close();
        setStatus('disconnected', '连接已暂停');
        await chrome.storage.local.set({ enabled: false });
        return { ok: true };
    }
    if (m.type === 'reconnect') {
        enabled = true;
        await chrome.storage.local.set({ enabled: true });
        restart();
        return { ok: true };
    }
    throw Error('不支持的操作');
}
function restart() { clearTimeout(reconnectTimer); clearInterval(heartbeat); cancelAll(); generation++; const old = socket; socket = null; authenticated = false; old?.close(); retry = 1000; void connect(); }
chrome.runtime.onMessage.addListener((message, sender, respond) => {
    if (sender.id !== chrome.runtime.id || sender.url !== chrome.runtime.getURL('popup.html'))
        return false;
    void dispatch(message).then(respond, error => respond({ error: error instanceof Error ? error.message : '请求失败' }));
    return true;
});
function setup() { void chrome.alarms.create('serialbridge-reconnect', { periodInMinutes: 1 }); void connect(); void chrome.storage.local.setAccessLevel?.({ accessLevel: 'TRUSTED_CONTEXTS' }); }
chrome.runtime.onInstalled.addListener(setup);
chrome.runtime.onStartup.addListener(setup);
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'serialbridge-reconnect')
    void connect(); });
setup();
