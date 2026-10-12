(()=>{const modules={};const cache={};modules['model']=function(module,exports,require){
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.deriveStatus = deriveStatus;
exports.filterLogs = filterLogs;
exports.progress = progress;
exports.escapeHtml = escapeHtml;
function deriveStatus(snapshot) {
    if (!snapshot || typeof snapshot !== 'object')
        return { core: false, peer: false, localBrowser: false, remoteBrowser: false };
    const s = snapshot;
    return { core: true, peer: s.peer?.connected === true, localBrowser: s.browser?.connected === true, remoteBrowser: s.peer?.connected === true && s.peer?.browser === true };
}
function filterLogs(logs, f) {
    return logs.filter(x => (!f.level || f.level === 'all' || x.level === f.level) && (!f.component || f.component === 'all' || x.component === f.component) && `${x.message} ${x.id ?? ''} ${x.component}`.toLowerCase().includes(f.query.toLowerCase()));
}
function progress(received, total) { return total && total > 0 ? Math.min(100, Math.max(0, 100 * received / total)) : null; }
function escapeHtml(x) { return String(x ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }

};
modules['types']=function(module,exports,require){
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });

};
modules['api']=function(module,exports,require){
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.native = exports.demo = void 0;
exports.configure = configure;
exports.hasCredentials = hasCredentials;
exports.api = api;
exports.saveContent = saveContent;
exports.pushFile = pushFile;
exports.demo = new URLSearchParams(location.search).get('demo') === '1';
exports.native = Boolean(window.__TAURI__);
function stored(key) {
    try {
        return sessionStorage.getItem(key);
    }
    catch {
        return null;
    }
}
let token = stored('serialbridge.token') ?? '';
let port = Number(stored('serialbridge.port') ?? 9527);
function configure(p, t) {
    if (!Number.isInteger(p) || p < 1 || p > 65535)
        throw Error('无效端口');
    port = p;
    token = t;
    try {
        sessionStorage.setItem('serialbridge.port', String(p));
        sessionStorage.setItem('serialbridge.token', t);
    }
    catch { /* Restricted webviews can still use in-memory credentials. */ }
}
function hasCredentials() { return exports.demo || exports.native || Boolean(token); }
async function api(method, path, body) {
    if (exports.demo)
        return demoApi(method, path, body);
    if (exports.native)
        return window.__TAURI__.core.invoke('api', { method, path, body: body ?? null });
    const response = await fetch(`http://127.0.0.1:${port}${path}`, { method, headers: { Authorization: `Bearer ${token}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(125000) });
    const data = await response.json();
    if (!response.ok)
        throw Error(data.error?.message ?? `HTTP ${response.status}`);
    return data;
}
async function saveContent(id, filename) {
    if (exports.demo)
        return '演示模式不生成真实下载文件';
    if (exports.native) {
        const r = await window.__TAURI__.core.invoke('file_location', { id });
        return `文件已保存在本机：\n${r.path}`;
    }
    const res = await fetch(`http://127.0.0.1:${port}/v1/downloads/${encodeURIComponent(id)}/content`, { headers: { Authorization: `Bearer ${token}` } });
    if (!res.ok)
        throw Error(`下载失败：HTTP ${res.status}`);
    const url = URL.createObjectURL(await res.blob());
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
async function pushFile(file, onProgress) {
    if (exports.demo)
        return await demoApi('POST', '/v1/downloads', { filename: file.name });
    if (exports.native) {
        const invoke = window.__TAURI__.core.invoke;
        let id = '';
        try {
            const start = await invoke('file_stage', { op: 'create', params: { filename: file.name, total_bytes: file.size }, data: [] });
            id = start.id;
            for (let offset = 0; offset < file.size; offset += 16384) {
                const data = new Uint8Array(await file.slice(offset, offset + 16384).arrayBuffer());
                await invoke('file_stage', { op: 'write', params: { id, offset }, data: Array.from(data) });
                onProgress(Math.min(offset + data.length, file.size));
            }
            return await invoke('file_stage', { op: 'finish', params: { id }, data: [] });
        }
        catch (e) {
            if (id)
                await invoke('file_stage', { op: 'cancel', params: { id }, data: [] }).catch(() => { });
            throw e;
        }
    }
    const res = await fetch(`http://127.0.0.1:${port}/v1/files/push/${encodeURIComponent(file.name)}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/octet-stream' }, body: file });
    const data = await res.json();
    if (!res.ok)
        throw Error(data.error?.message ?? '上传失败');
    return data;
}
// This adapter is opt-in only (?demo=1). It is never selected on connection failure.
let demoOnline = true;
const demoSettings = { node_name: 'Node A', serial_port: '/dev/cu.usbserial-DEMO-A', baud_rate: 921600, http_port: 9527, allowed_origins: ['https://example.com'], allowed_methods: ['GET', 'HEAD'], auto_reconnect: true, download_dir: '~/SerialBridge/downloads', shared_dir: '~/SerialBridge/shared', max_file_bytes: 104857600, max_spool_bytes: 1073741824, chunk_bytes: 8192 };
const demoTransfers = [{ id: 'demo-complete', filename: 'interface-report.json', direction: 'receive', target: 'peer', status: 'completed', stage: 'completed', received_bytes: 48128, total_bytes: 48128, sha256: 'demo-not-a-real-checksum', created_at: Date.now() - 240000 }, { id: 'demo-progress', filename: 'reference-package.zip', direction: 'receive', target: 'peer', status: 'running', stage: 'serial_transfer', received_bytes: 786432, total_bytes: 2097152, created_at: Date.now() - 90000 }];
async function demoApi(method, path, body) {
    const p = body;
    if (path === '/v1/health')
        return { version: '0.1.0-alpha.1', node: { name: demoSettings.node_name, os: 'macos' }, http: { port: 9527 }, serial: { port: demoSettings.serial_port, baud_rate: demoSettings.baud_rate }, peer: { connected: demoOnline, name: 'Node B', os: 'windows', browser: demoOnline }, browser: { connected: true, origin: 'chrome-extension://demo' }, pending_requests: 0 };
    if (path === '/v1/settings') {
        if (method === 'PUT') {
            Object.assign(demoSettings, p);
            return { settings: demoSettings, restart_required: false };
        }
        return demoSettings;
    }
    if (path === '/v1/ports')
        return { ports: [{ name: '/dev/cu.usbserial-DEMO-A', product: 'FT232RL · 演示串口' }] };
    if (path === '/v1/logs')
        return { logs: [{ time: Date.now() - 3100, level: 'info', component: 'serial', message: '[演示] Peer handshake complete', id: 'demo-01' }, { time: Date.now() - 2200, level: 'info', component: 'browser', message: '[演示] Local extension authenticated', id: 'demo-02' }, { time: Date.now() - 1100, level: 'warn', component: 'rpc', message: '[演示] ORIGIN_NOT_ALLOWED — 请求被策略拒绝', id: 'demo-03' }, { time: Date.now() - 200, level: 'info', component: 'transfer', message: '[演示] File received; SHA-256 matches', id: 'demo-04' }] };
    if (path === '/v1/downloads' && method === 'GET')
        return { transfers: demoTransfers };
    if ((path === '/v1/downloads' || path === '/v1/files/pull') && method === 'POST') {
        const t = { id: `demo-${Date.now()}`, filename: String(p?.filename ?? 'download.bin'), direction: 'receive', target: 'peer', status: 'queued', stage: 'queued', received_bytes: 0, total_bytes: null, created_at: Date.now() };
        demoTransfers.unshift(t);
        return t;
    }
    if (path.startsWith('/v1/downloads/') && method === 'DELETE') {
        const id = path.split('/')[3];
        const t = demoTransfers.find(t => t.id === id);
        if (t) {
            if (['completed', 'failed', 'cancelled'].includes(t.status))
                demoTransfers.splice(demoTransfers.indexOf(t), 1);
            else
                t.status = 'cancelled';
        }
        return { cancelling: true };
    }
    if (path === '/v1/extension/pairing')
        return method === 'DELETE' ? { revoked: true } : { code: 'DEMO00000000', expires_in_seconds: 180 };
    if (path === '/v1/serial/reconnect') {
        demoOnline = !demoOnline;
        return { reconnecting: true };
    }
    if (path === '/v1/rpc') {
        if (!demoOnline && p?.target !== 'local')
            throw Error('演示：对端未连接');
        return { result: { name: p?.target === 'local' ? 'Node A' : 'Node B', protocol: 1, demo: true } };
    }
    throw Error('演示模式未提供这个操作');
}

};
modules['main']=function(module,exports,require){
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const api_js_1 = require("./api.js");
const model_js_1 = require("./model.js");
const state = { snapshot: null, logs: [], transfers: [], settings: null, ports: [], page: 'overview', paused: false, filter: { level: 'all', component: 'all', query: '' }, error: '' };
const labels = { overview: '总览', logs: '实时日志', transfers: '文件传输', settings: '设置' };
function el(id) {
    const x = document.getElementById(id);
    if (!x)
        throw Error(`Missing element: ${id}`);
    return x;
}
let toastTimer;
function toast(message) { el('toast').textContent = message; el('toast').classList.remove('hidden'); clearTimeout(toastTimer); toastTimer = setTimeout(() => el('toast').classList.add('hidden'), 5500); }
function fail(err) { toast(err instanceof Error ? err.message : String(err)); }
function bytes(n) {
    if (n < 1024)
        return `${n} B`;
    if (n < 1024 ** 2)
        return `${(n / 1024).toFixed(1)} KiB`;
    return `${(n / 1024 ** 2).toFixed(1)} MiB`;
}
function time(t) { return t ? new Date(t).toLocaleTimeString('zh-CN', { hour12: false }) : '—'; }
function badge(on, label) { return `<span class="pill ${on ? '' : 'neutral'}"><span class="dot ${on ? '' : 'bad'}"></span>${(0, model_js_1.escapeHtml)(label ?? (on ? '已连接' : '未连接'))}</span>`; }
function modal(title, html) {
    el('dialog-title').textContent = title;
    el('dialog-content').innerHTML = html;
    const d = el('dialog');
    if (!d.open)
        d.showModal();
}
function head(title, sub, actions = '') { return `<div class="page-head"><div><div class="eyebrow">SERIALBRIDGE / ${state.page.toUpperCase()}</div><h1 class="page-title">${title}</h1><p class="page-sub">${sub}</p></div><div class="page-head-right">${actions}</div></div>`; }
function topStatus() {
    const s = state.snapshot;
    const live = (0, model_js_1.deriveStatus)(s);
    el('side-name').textContent = s?.node.name ?? '本机服务未连接';
    el('side-dot').className = `dot ${live.core ? '' : 'bad'}`;
    el('side-detail').textContent = s ? `${s.node.os} · 127.0.0.1:${s.http.port}` : '检查进程、端口和访问凭据';
    el('connection-pill').className = `pill ${live.core ? '' : 'neutral'}`;
    el('connection-pill').textContent = live.core ? (live.peer ? '本机与对端已连接' : '本机在线 · 对端未连接') : '本机服务未连接';
    el('error-banner').classList.toggle('hidden', !state.error);
    el('error-banner').textContent = state.error;
}
function renderOverview() {
    const s = state.snapshot;
    const st = (0, model_js_1.deriveStatus)(s);
    const cards = [['本机 HTTP 服务', s ? `127.0.0.1:${s.http.port}` : '未连接', '统一的本机调用入口', st.core], ['串口对端', s?.peer.connected ? s.peer.name ?? '对端节点' : '未连接', s?.serial.port ?? '尚未选择串口', st.peer], ['本机浏览器扩展', st.localBrowser ? '已连接' : '未连接', '执行本节点的浏览器请求', st.localBrowser], ['对端浏览器扩展', st.peer ? (st.remoteBrowser ? '已连接' : '未连接') : '状态未知', st.peer ? '通过串口同步对端状态' : '请先连接对端节点', st.remoteBrowser]];
    el('page-content').innerHTML = head('连接，一目了然。', '任意一端都可以发起请求。节点角色不绑定操作系统。') + `<div class="stats">${cards.map(c => `<section class="card stat"><div class="stat-head"><span class="stat-label">${(0, model_js_1.escapeHtml)(c[0])}</span><span class="dot ${c[3] ? '' : 'bad'}"></span></div><div class="stat-value">${(0, model_js_1.escapeHtml)(c[1])}</div><div class="stat-sub">${(0, model_js_1.escapeHtml)(c[2])}</div></section>`).join('')}</div>
 <section class="card"><div class="section-head"><div><h2>双端连接拓扑</h2><div class="section-desc">HTTP → 本机节点 ⇄ USB 串口 ⇄ 对端节点 → 浏览器扩展</div></div><span class="tag">FT232RL / VCP</span></div><div class="live-topology"><div class="live-node"><span class="tag">本机节点</span><h3>${(0, model_js_1.escapeHtml)(s?.node.name ?? '当前电脑')}</h3><small>${(0, model_js_1.escapeHtml)(s?.node.os ?? '等待服务')} · 对等节点</small><div class="node-detail"><span>本机扩展</span>${badge(st.localBrowser)}</div></div><div class="bridge-cable"><b>${st.peer ? '双向通信' : '等待连接'}</b><div class="bridge-line ${st.peer ? 'on' : ''}"></div><span>${(0, model_js_1.escapeHtml)(s?.serial.baud_rate ?? 115200)} baud · 8N1</span><span>${s?.pending_requests ?? 0} 个等待中的请求</span></div><div class="live-node"><span class="tag">对端节点</span><h3>${(0, model_js_1.escapeHtml)(st.peer ? s?.peer.name : '尚未发现对端')}</h3><small>${(0, model_js_1.escapeHtml)(st.peer ? s?.peer.os : 'Windows / macOS')} · 对等节点</small><div class="node-detail"><span>对端扩展</span>${badge(st.remoteBrowser, st.peer ? undefined : '未知')}</div></div></div><div class="quick-row"><button class="btn" id="test-local">测试本机</button><button class="btn" id="reconnect-peer">重新连接串口</button><button class="btn" id="go-settings">配置连接</button>${!api_js_1.native && !api_js_1.demo ? '<button class="btn" id="open-login">更换 API 凭据</button>' : ''}</div></section>
 <section class="card" style="margin-top:18px"><div class="section-head"><div><h2>最近活动</h2><div class="section-desc">只显示真实事件；没有事件时不会填充演示数据。</div></div><button class="ghost-link" id="go-logs">查看全部 →</button></div>${state.logs.length ? [...state.logs].slice(-4).reverse().map(l => `<div class="activity-snippet"><span class="mono subtle">${time(l.time)}</span><span class="level ${(0, model_js_1.escapeHtml)(l.level)}">${(0, model_js_1.escapeHtml)(l.component)}</span><span class="message">${(0, model_js_1.escapeHtml)(l.message)}</span></div>`).join('') : '<div class="empty-state">暂无事件。连接服务后，日志会显示在这里。</div>'}</section>`;
    el('test-local').onclick = () => void ping('local');
    el('reconnect-peer').onclick = () => void (0, api_js_1.api)('POST', '/v1/serial/reconnect', {}).then(() => { toast('已请求重新连接串口'); void refresh(); }).catch(fail);
    el('go-settings').onclick = () => void navigate('settings');
    el('go-logs').onclick = () => void navigate('logs');
    document.getElementById('open-login')?.addEventListener('click', () => el('connect-dialog').showModal());
}
function renderLogs() {
    el('page-content').innerHTML = head('实时日志', '按模块、级别或请求 ID 定位问题。敏感请求正文与凭据不会写入日志。', `<button class="btn" id="pause-logs">${state.paused ? '继续接收' : '暂停滚动'}</button><button class="btn" id="export-logs">导出日志</button>`) + `<section class="card"><div class="toolbar"><label class="sr-only" for="log-query">搜索日志</label><input id="log-query" placeholder="搜索关键词 / 请求 ID" value="${(0, model_js_1.escapeHtml)(state.filter.query)}"><select id="log-level" aria-label="日志级别"><option value="all">全部级别</option><option value="info">Info</option><option value="warn">Warn</option><option value="error">Error</option></select><select id="log-component" aria-label="日志模块"><option value="all">全部模块</option>${['serial', 'rpc', 'browser', 'transfer', 'http', 'settings'].map(m => `<option>${m}</option>`).join('')}</select><span id="log-count" class="subtle"></span></div><div class="table-wrap"><table class="live-table"><thead><tr><th>时间</th><th>级别</th><th>模块</th><th>事件</th><th>请求 ID</th></tr></thead><tbody id="log-rows"></tbody></table></div></section>`;
    el('log-level').value = state.filter.level;
    el('log-component').value = state.filter.component;
    el('log-query').oninput = () => { state.filter.query = el('log-query').value; updateLogRows(); };
    el('log-level').onchange = () => { state.filter.level = el('log-level').value; updateLogRows(); };
    el('log-component').onchange = () => { state.filter.component = el('log-component').value; updateLogRows(); };
    el('pause-logs').onclick = () => { state.paused = !state.paused; el('pause-logs').textContent = state.paused ? '继续接收' : '暂停滚动'; };
    el('export-logs').onclick = () => void exportLogs();
    updateLogRows();
}
function updateLogRows() {
    const rows = (0, model_js_1.filterLogs)(state.logs, state.filter).reverse();
    el('log-count').textContent = `${rows.length} 条`;
    el('log-rows').innerHTML = rows.length ? rows.map((l, i) => `<tr data-log="${i}" tabindex="0"><td class="mono subtle">${time(l.time)}</td><td><span class="level ${(0, model_js_1.escapeHtml)(l.level)}">${(0, model_js_1.escapeHtml)(l.level.toUpperCase())}</span></td><td>${(0, model_js_1.escapeHtml)(l.component)}</td><td>${(0, model_js_1.escapeHtml)(l.message)}</td><td class="mono subtle">${(0, model_js_1.escapeHtml)((l.id ?? '').slice(0, 12) || '—')}</td></tr>`).join('') : '<tr><td colspan="5"><div class="empty-state">没有符合筛选条件的日志</div></td></tr>';
    document.querySelectorAll('[data-log]').forEach(row => {
        const show = () => modal('日志详情', `<pre>${(0, model_js_1.escapeHtml)(JSON.stringify(rows[Number(row.dataset.log)], null, 2))}</pre>`);
        row.onclick = show;
        row.onkeydown = ev => {
            if (ev.key === 'Enter')
                show();
        };
    });
}
async function exportLogs() {
    try {
        if (api_js_1.native && !api_js_1.demo) {
            const v = await window.__TAURI__.core.invoke('export_logs');
            toast(`日志已写入 ${v.path}`);
            return;
        }
        const logs = (0, model_js_1.filterLogs)(state.logs, state.filter);
        const u = URL.createObjectURL(new Blob([logs.map(l => JSON.stringify(l)).join('\n')], { type: 'application/x-ndjson' }));
        const a = document.createElement('a');
        a.href = u;
        a.download = 'serialbridge-logs.jsonl';
        a.click();
        setTimeout(() => URL.revokeObjectURL(u), 1000);
    }
    catch (err) {
        fail(err);
    }
}
const statusNames = { queued: '排队中', running: '传输中', completed: '已完成', failed: '失败', cancelled: '已取消' };
const stages = { queued: '等待执行', source_download: '执行端下载', local_staging: '本机上传暂存', serial_transfer: '串口传输', completed: '完整性校验通过' };
function renderTransfers() {
    el('page-content').innerHTML = head('文件传输', '执行端下载 → 串口分块传输 → 接收端完整性校验。', `<button class="btn" id="pull-file">拉取对端文件</button><button class="btn" id="push-file">推送文件</button><button class="btn primary" id="new-download">新建下载</button>`) + `<div id="transfer-summary" class="runtime-note"></div><div id="transfer-list"></div>`;
    el('new-download').onclick = () => downloadDialog();
    el('pull-file').onclick = () => pullDialog();
    el('push-file').onclick = () => el('push-picker').click();
    updateTransfers();
}
function updateTransfers() {
    el('transfer-summary').textContent = `${state.transfers.filter(t => ['running', 'queued'].includes(t.status)).length} 个进行中 · ${state.transfers.filter(t => t.status === 'completed').length} 个已完成 · 文件传输不会自动提升串口速率`;
    el('transfer-list').innerHTML = state.transfers.length ? state.transfers.map(t => {
        const p = (0, model_js_1.progress)(t.received_bytes, t.total_bytes);
        const done = t.status === 'completed';
        const active = ['running', 'queued'].includes(t.status);
        return `<article class="card transfer-live"><div class="file-mark">${t.direction === 'send' ? '↗' : '↙'}</div><div class="file-main"><h3>${(0, model_js_1.escapeHtml)(t.filename)} <span class="tag">${(0, model_js_1.escapeHtml)(statusNames[t.status] ?? t.status)}</span></h3><div class="subtle" style="font-size:11px">${(0, model_js_1.escapeHtml)(t.direction === 'send' ? '本机 → 对端' : t.direction === 'staging' ? '本机上传暂存' : t.target === 'local' ? '本机下载' : '对端 → 本机')} · ${(0, model_js_1.escapeHtml)(stages[t.stage] ?? t.stage)}</div><div class="progress-track"><div class="progress-fill ${p === null ? 'unknown' : ''}" style="width:${done ? 100 : p ?? 20}%"></div></div><div class="file-info"><span>${bytes(t.received_bytes)} / ${t.total_bytes === null ? '大小未知' : bytes(t.total_bytes)}</span><span>${done ? 'SHA-256 已校验' : p === null ? '等待数据' : `${p.toFixed(1)}%`}</span></div>${t.error ? `<p class="warning-copy break">${(0, model_js_1.escapeHtml)(t.error)}</p>` : ''}</div><div class="actions">${done && t.direction !== 'send' ? `<button class="btn small" data-save="${(0, model_js_1.escapeHtml)(t.id)}">${api_js_1.native ? '文件位置' : '保存文件'}</button>` : ''}<button class="btn small" data-transfer="${(0, model_js_1.escapeHtml)(t.id)}">详情</button><button class="btn small ${active ? '' : 'quiet'}" data-cancel="${(0, model_js_1.escapeHtml)(t.id)}">${active ? '取消' : '移除'}</button></div></article>`;
    }).join('') : '<div class="card empty-state">暂无文件任务。新建下载或选择一个文件开始传输。</div>';
    document.querySelectorAll('[data-transfer]').forEach(b => b.onclick = () => { const t = state.transfers.find(t => t.id === b.dataset.transfer); modal('传输详情', `<pre>${(0, model_js_1.escapeHtml)(JSON.stringify(t, null, 2))}</pre>`); });
    document.querySelectorAll('[data-cancel]').forEach(b => b.onclick = () => void (0, api_js_1.api)('DELETE', `/v1/downloads/${b.dataset.cancel}`).then(() => { toast('操作已提交'); void refresh(); }).catch(fail));
    document.querySelectorAll('[data-save]').forEach(b => b.onclick = () => {
        const t = state.transfers.find(t => t.id === b.dataset.save);
        void (0, api_js_1.saveContent)(t.id, t.filename).then(message => {
            if (message)
                modal('文件位置', `<pre>${(0, model_js_1.escapeHtml)(message)}</pre>`);
        }).catch(fail);
    });
}
function downloadDialog() {
    modal('新建文件下载', `<form id="download-form"><div class="form-row"><label class="form-label">执行位置<select name="target"><option value="peer">对端节点</option><option value="local">本机节点</option></select></label><label class="form-label">执行器<select name="executor"><option value="browser">浏览器扩展</option><option value="http">普通 HTTP</option></select></label></div><label class="form-label">最终下载 URL<input name="url" type="url" required placeholder="https://example.com/files/report.zip"></label><label class="form-label">文件名<input name="filename" required value="report.zip" maxlength="120"></label><p class="form-hint">目标来源必须被执行节点允许；浏览器执行器还需扩展网站授权。Alpha 不跟随重定向。</p><button class="btn primary" type="submit">创建下载</button></form>`);
    el('download-form').onsubmit = event => { event.preventDefault(); const form = el('download-form'); const data = Object.fromEntries(new FormData(form)); const button = form.querySelector('button'); button.disabled = true; void (0, api_js_1.api)('POST', '/v1/downloads', data).then(() => { el('dialog').close(); toast('下载任务已创建'); void refresh(); }).catch(fail).finally(() => button.disabled = false); };
}
function pullDialog() { modal('从对端共享目录拉取文件', `<form id="pull-form"><label class="form-label">共享目录中的文件名<input name="filename" placeholder="report.zip" required maxlength="120"></label><p class="form-hint">Alpha 只允许共享根目录下的普通文件名，不接收任意磁盘路径。</p><button class="btn primary" type="submit">拉取文件</button></form>`); el('pull-form').onsubmit = event => { event.preventDefault(); const data = Object.fromEntries(new FormData(el('pull-form'))); void (0, api_js_1.api)('POST', '/v1/files/pull', data).then(() => { el('dialog').close(); void refresh(); }).catch(fail); }; }
function renderSettings() {
    const c = state.settings;
    if (!c) {
        el('page-content').innerHTML = head('设置', '只配置需要的选项。') + `<section class="card empty-state">尚未读取到服务配置。<br><button id="settings-login" class="btn connection-entry">连接本机服务</button></section>`;
        el('settings-login').onclick = () => {
            if (api_js_1.native)
                void refresh();
            else
                el('connect-dialog').showModal();
        };
        return;
    }
    el('page-content').innerHTML = head('设置', '两端使用相同配置结构；串口、目标权限和配对分别管理。') + `<div class="settings-grid-live"><form id="settings-form"><section class="card settings-box"><h2>节点与串口</h2><label class="form-label">节点名称<input name="node_name" value="${(0, model_js_1.escapeHtml)(c.node_name)}" maxlength="80" required></label><label class="form-label">串口<input name="serial_port" list="port-options" value="${(0, model_js_1.escapeHtml)(c.serial_port)}" placeholder="COM5 或 /dev/cu.usbserial-…"><datalist id="port-options">${state.ports.map(p => `<option value="${(0, model_js_1.escapeHtml)(p.name)}">${(0, model_js_1.escapeHtml)(p.product ?? '')}</option>`).join('')}</datalist></label><div class="form-row"><label class="form-label">波特率<select name="baud_rate">${[9600, 57600, 115200, 460800, 921600, 1000000, 2000000, 3000000].map(n => `<option value="${n}" ${n === c.baud_rate ? 'selected' : ''}>${n}</option>`).join('')}</select></label><label class="form-label">本地 HTTP 端口<input name="http_port" type="number" min="1" max="65535" value="${c.http_port}" required></label></div><label class="form-label"><input name="auto_reconnect" type="checkbox" style="display:inline;width:auto;margin-right:7px" ${c.auto_reconnect ? 'checked' : ''}>断开后自动重连</label><p class="form-hint">更改串口会重建链路。更改 HTTP 端口需要重启程序。两端波特率必须一致；初次验证建议 115200。</p></section><section class="card settings-box"><h2>允许执行的 HTTP 请求</h2><label class="form-label">来源白名单，每行一个<textarea name="allowed_origins" rows="4" placeholder="https://example.com">${(0, model_js_1.escapeHtml)(c.allowed_origins.join('\n'))}</textarea></label><label class="form-label">允许的方法<input name="allowed_methods" value="${(0, model_js_1.escapeHtml)(c.allowed_methods.join(', '))}" required></label><p class="form-hint">精确到协议、主机与端口，不填写路径或通配符。留空表示全部拒绝。POST 等写入操作需明确启用。</p></section><div class="settings-footer"><button class="btn primary" type="submit">保存设置</button><span class="form-hint">仅修改本机配置，不替对端保存。</span></div></form><div><section class="card settings-box"><h2>浏览器扩展</h2><div id="settings-browser">${badge(Boolean(state.snapshot?.browser.connected))}</div><p class="form-hint">扩展主动连接本机；配对凭据不会通过串口发送。</p><div id="pair-result"></div><div class="quick-row" style="padding:10px 0 0"><button class="btn primary" id="new-pair">生成配对码</button><button class="btn danger" id="revoke-pair">解除配对</button></div><p class="form-hint">在扩展弹窗中输入端口与配对码。代码有效期 3 分钟，最多尝试 5 次。</p></section><section class="card settings-box"><h2>文件与日志</h2><label class="form-label">下载目录<div class="mono break form-hint">${(0, model_js_1.escapeHtml)(c.download_dir)}</div></label><label class="form-label">共享目录<div class="mono break form-hint">${(0, model_js_1.escapeHtml)(c.shared_dir)}</div></label><p class="form-hint">单文件上限 ${bytes(c.max_file_bytes)} · 目录配额 ${bytes(c.max_spool_bytes)}<br>日志保留最近 2000 条，可在日志页导出。目录路径请在停止程序后修改 TOML 配置。</p></section><section class="card settings-box"><h2>Alpha 边界</h2><p class="form-hint">没有硬件时不显示虚构连接。浏览器关闭后相关任务失败，不偷偷改为普通 HTTP。当前没有开机自启、断点续传或多浏览器配置。</p>${!api_js_1.native ? '<button class="btn" id="settings-token">更换 API 凭据</button>' : ''}</section></div></div>`;
    el('settings-form').onsubmit = event => { event.preventDefault(); const form = el('settings-form'); const f = new FormData(form); const patch = { node_name: String(f.get('node_name')), serial_port: String(f.get('serial_port')), baud_rate: Number(f.get('baud_rate')), http_port: Number(f.get('http_port')), auto_reconnect: f.get('auto_reconnect') === 'on', allowed_origins: String(f.get('allowed_origins')).split(/\r?\n/).map(s => s.trim()).filter(Boolean), allowed_methods: String(f.get('allowed_methods')).split(/[\s,]+/).map(s => s.toUpperCase()).filter(Boolean) }; const button = form.querySelector('button[type=submit]'); button.disabled = true; void (0, api_js_1.api)('PUT', '/v1/settings', patch).then(r => { state.settings = r.settings; toast(r.restart_required ? '配置已保存。HTTP 端口变更需重启后生效。' : '设置已保存'); void refresh(); }).catch(fail).finally(() => button.disabled = false); };
    el('new-pair').onclick = () => void (0, api_js_1.api)('POST', '/v1/extension/pairing', {}).then(r => { el('pair-result').innerHTML = `<div class="pair-code">${(0, model_js_1.escapeHtml)(r.code)}</div><p class="form-hint">请在 ${r.expires_in_seconds} 秒内输入扩展。不要把此配对码发给其他人。</p>`; }).catch(fail);
    el('revoke-pair').onclick = () => void (0, api_js_1.api)('DELETE', '/v1/extension/pairing').then(() => { el('pair-result').innerHTML = ''; toast('本机扩展配对已解除'); void refresh(); }).catch(fail);
    document.getElementById('settings-token')?.addEventListener('click', () => el('connect-dialog').showModal());
}
async function navigate(page) {
    if (!labels[page])
        return;
    state.page = page;
    el('page-title').textContent = labels[page];
    document.querySelectorAll('[data-page]').forEach(b => b.classList.toggle('active', b.dataset.page === page));
    if (page === 'settings' && (0, api_js_1.hasCredentials)()) {
        try {
            const [c, p] = await Promise.all([(0, api_js_1.api)('GET', '/v1/settings'), (0, api_js_1.api)('GET', '/v1/ports')]);
            state.settings = c;
            state.ports = p.ports;
        }
        catch (err) {
            fail(err);
        }
    }
    renderPage();
}
function renderPage() {
    if (state.page === 'overview')
        renderOverview();
    else if (state.page === 'logs')
        renderLogs();
    else if (state.page === 'transfers')
        renderTransfers();
    else
        renderSettings();
}
let loading = false;
async function refresh() {
    if (loading || !(0, api_js_1.hasCredentials)())
        return;
    loading = true;
    try {
        const [s, l, t] = await Promise.all([(0, api_js_1.api)('GET', '/v1/health'), (0, api_js_1.api)('GET', '/v1/logs'), (0, api_js_1.api)('GET', '/v1/downloads')]);
        state.snapshot = s;
        if (!state.paused)
            state.logs = l.logs;
        state.transfers = t.transfers;
        state.error = '';
    }
    catch (err) {
        state.snapshot = null;
        state.error = `本机服务未连接：${err instanceof Error ? err.message : String(err)}。界面不会自动切换为演示数据。`;
    }
    finally {
        loading = false;
        topStatus();
        if (state.page === 'overview')
            renderOverview();
        else if (state.page === 'logs')
            updateLogRows();
        else if (state.page === 'transfers')
            updateTransfers();
        else if (document.getElementById('settings-browser'))
            el('settings-browser').innerHTML = badge(Boolean(state.snapshot?.browser.connected));
    }
}
async function ping(target = 'peer') {
    try {
        const before = performance.now();
        const r = await (0, api_js_1.api)('POST', '/v1/rpc', { target, method: 'system.ping', params: {} });
        modal(`${target === 'peer' ? '对端' : '本机'}测试结果`, `${api_js_1.demo ? '<p class="form-hint">模拟请求，不代表真实串口往返时间。</p>' : `<p class="form-hint">本次调用耗时 ${Math.round(performance.now() - before)} ms</p>`}<pre>${(0, model_js_1.escapeHtml)(JSON.stringify(r, null, 2))}</pre>`);
        void refresh();
    }
    catch (err) {
        fail(err);
    }
}
el('close-dialog').onclick = () => el('dialog').close();
el('close-connect').onclick = () => el('connect-dialog').close();
el('refresh').onclick = () => void refresh();
el('ping').onclick = () => void ping();
for (const nav of document.querySelectorAll('[data-page]'))
    nav.onclick = () => void navigate(nav.dataset.page);
el('connect-form').onsubmit = event => {
    event.preventDefault();
    try {
        (0, api_js_1.configure)(Number(el('api-port').value), el('api-token').value.trim());
        el('api-token').value = '';
        el('connect-dialog').close();
        void refresh().then(() => {
            if (state.page === 'settings')
                void navigate('settings');
        });
    }
    catch (err) {
        fail(err);
    }
};
el('push-picker').onchange = () => {
    const input = el('push-picker');
    const file = input.files?.[0];
    if (!file)
        return;
    toast(`正在准备 ${file.name}`);
    void (0, api_js_1.pushFile)(file, n => toast(`上传至本机节点：${bytes(n)} / ${bytes(file.size)}`)).then(() => { toast('文件已进入对端传输队列'); void navigate('transfers'); void refresh(); }).catch(fail).finally(() => input.value = '');
};
el('run-mode').textContent = api_js_1.demo ? 'DEMO' : api_js_1.native ? 'NATIVE' : 'HTTP';
el('demo-banner').classList.toggle('hidden', !api_js_1.demo);
renderPage();
topStatus();
if (!(0, api_js_1.hasCredentials)())
    el('connect-dialog').showModal();
else
    void refresh();
setInterval(() => void refresh(), 1500);

};
function require(id){id=id.replace('./','').replace('.js','');if(cache[id])return cache[id].exports;const module={exports:{}};cache[id]=module;modules[id](module,module.exports,require);return module.exports;}require('main');})();