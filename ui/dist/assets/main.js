import { api, configure, demo, hasCredentials, native, pushFile, saveContent } from './api.js';
import { deriveStatus, escapeHtml as e, filterLogs, progress } from './model.js';
const state = { snapshot: null, logs: [], transfers: [], tunnels: null, settings: null, ports: [], page: 'overview', paused: false, filter: { level: 'all', component: 'all', query: '' }, error: '' };
const labels = { overview: '总览', logs: '实时日志', transfers: '文件传输', tunnels: 'TCP 转发', settings: '设置' };
function el(id) { const x = document.getElementById(id); if (!x)
    throw Error(`Missing element: ${id}`); return x; }
let toastTimer;
function toast(message) { el('toast').textContent = message; el('toast').classList.remove('hidden'); clearTimeout(toastTimer); toastTimer = setTimeout(() => el('toast').classList.add('hidden'), 5500); }
function fail(err) { toast(err instanceof Error ? err.message : String(err)); }
function bytes(n) { if (n < 1024)
    return `${n} B`; if (n < 1024 ** 2)
    return `${(n / 1024).toFixed(1)} KiB`; return `${(n / 1024 ** 2).toFixed(1)} MiB`; }
function time(t) { return t ? new Date(t).toLocaleTimeString('zh-CN', { hour12: false }) : '—'; }
function badge(on, label) { return `<span class="pill ${on ? '' : 'neutral'}"><span class="dot ${on ? '' : 'bad'}"></span>${e(label ?? (on ? '已连接' : '未连接'))}</span>`; }
function modal(title, html) { el('dialog-title').textContent = title; el('dialog-content').innerHTML = html; const d = el('dialog'); if (!d.open)
    d.showModal(); }
function head(title, sub, actions = '') { return `<div class="page-head"><div><div class="eyebrow">SERIALBRIDGE / ${state.page.toUpperCase()}</div><h1 class="page-title">${title}</h1><p class="page-sub">${sub}</p></div><div class="page-head-right">${actions}</div></div>`; }
function topStatus() {
    const s = state.snapshot;
    const live = deriveStatus(s);
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
    const st = deriveStatus(s);
    const cards = [['本机 HTTP 服务', s ? `127.0.0.1:${s.http.port}` : '未连接', '统一的本机调用入口', st.core], ['串口对端', s?.peer.connected ? s.peer.name ?? '对端节点' : '未连接', s?.serial.port ?? '尚未选择串口', st.peer], ['TCP 转发', `${s?.tunnel?.active_streams ?? 0} 个连接`, `${s?.tunnel?.forwards ?? 0} 条转发规则`, Boolean(s?.tunnel?.active_streams)], ['本机浏览器扩展', st.localBrowser ? '已连接' : '未连接', '执行本节点的浏览器请求', st.localBrowser], ['对端浏览器扩展', st.peer ? (st.remoteBrowser ? '已连接' : '未连接') : '状态未知', st.peer ? '通过串口同步对端状态' : '请先连接对端节点', st.remoteBrowser]];
    el('page-content').innerHTML = head('连接，一目了然。', '任意一端都可以发起请求。节点角色不绑定操作系统。') + `<div class="stats">${cards.map(c => `<section class="card stat"><div class="stat-head"><span class="stat-label">${e(c[0])}</span><span class="dot ${c[3] ? '' : 'bad'}"></span></div><div class="stat-value">${e(c[1])}</div><div class="stat-sub">${e(c[2])}</div></section>`).join('')}</div>
 <section class="card"><div class="section-head"><div><h2>双端连接拓扑</h2><div class="section-desc">HTTP → 本机节点 ⇄ USB 串口 ⇄ 对端节点 → 浏览器扩展</div></div><span class="tag">FT232RL / VCP</span></div><div class="live-topology"><div class="live-node"><span class="tag">本机节点</span><h3>${e(s?.node.name ?? '当前电脑')}</h3><small>${e(s?.node.os ?? '等待服务')} · 对等节点</small><div class="node-detail"><span>本机扩展</span>${badge(st.localBrowser)}</div></div><div class="bridge-cable"><b>${st.peer ? '双向通信' : '等待连接'}</b><div class="bridge-line ${st.peer ? 'on' : ''}"></div><span>${e(s?.serial.baud_rate ?? 115200)} baud · 8N1</span><span>${s?.pending_requests ?? 0} 个等待中的请求</span></div><div class="live-node"><span class="tag">对端节点</span><h3>${e(st.peer ? s?.peer.name : '尚未发现对端')}</h3><small>${e(st.peer ? s?.peer.os : 'Windows / macOS')} · 对等节点</small><div class="node-detail"><span>对端扩展</span>${badge(st.remoteBrowser, st.peer ? undefined : '未知')}</div></div></div><div class="quick-row"><button class="btn" id="test-local">测试本机</button><button class="btn" id="reconnect-peer">重新连接串口</button><button class="btn" id="go-settings">配置连接</button>${!native && !demo ? '<button class="btn" id="open-login">更换 API 凭据</button>' : ''}</div></section>
 <section class="card" style="margin-top:18px"><div class="section-head"><div><h2>最近活动</h2><div class="section-desc">只显示真实事件；没有事件时不会填充演示数据。</div></div><button class="ghost-link" id="go-logs">查看全部 →</button></div>${state.logs.length ? [...state.logs].slice(-4).reverse().map(l => `<div class="activity-snippet"><span class="mono subtle">${time(l.time)}</span><span class="level ${e(l.level)}">${e(l.component)}</span><span class="message">${e(l.message)}</span></div>`).join('') : '<div class="empty-state">暂无事件。连接服务后，日志会显示在这里。</div>'}</section>`;
    el('test-local').onclick = () => void ping('local');
    el('reconnect-peer').onclick = () => void api('POST', '/v1/serial/reconnect', {}).then(() => { toast('已请求重新连接串口'); void refresh(); }).catch(fail);
    el('go-settings').onclick = () => void navigate('settings');
    el('go-logs').onclick = () => void navigate('logs');
    document.getElementById('open-login')?.addEventListener('click', () => el('connect-dialog').showModal());
}
function renderLogs() {
    el('page-content').innerHTML = head('实时日志', '按模块、级别或请求 ID 定位问题。敏感请求正文与凭据不会写入日志。', `<button class="btn" id="pause-logs">${state.paused ? '继续接收' : '暂停滚动'}</button><button class="btn" id="export-logs">导出日志</button>`) + `<section class="card"><div class="toolbar"><label class="sr-only" for="log-query">搜索日志</label><input id="log-query" placeholder="搜索关键词 / 请求 ID" value="${e(state.filter.query)}"><select id="log-level" aria-label="日志级别"><option value="all">全部级别</option><option value="info">Info</option><option value="warn">Warn</option><option value="error">Error</option></select><select id="log-component" aria-label="日志模块"><option value="all">全部模块</option>${['serial', 'rpc', 'browser', 'transfer', 'tunnel', 'http', 'settings'].map(m => `<option>${m}</option>`).join('')}</select><span id="log-count" class="subtle"></span></div><div class="table-wrap"><table class="live-table"><thead><tr><th>时间</th><th>级别</th><th>模块</th><th>事件</th><th>请求 ID</th></tr></thead><tbody id="log-rows"></tbody></table></div></section>`;
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
    const rows = filterLogs(state.logs, state.filter).reverse();
    el('log-count').textContent = `${rows.length} 条`;
    el('log-rows').innerHTML = rows.length ? rows.map((l, i) => `<tr data-log="${i}" tabindex="0"><td class="mono subtle">${time(l.time)}</td><td><span class="level ${e(l.level)}">${e(l.level.toUpperCase())}</span></td><td>${e(l.component)}</td><td>${e(l.message)}</td><td class="mono subtle">${e((l.id ?? '').slice(0, 12) || '—')}</td></tr>`).join('') : '<tr><td colspan="5"><div class="empty-state">没有符合筛选条件的日志</div></td></tr>';
    document.querySelectorAll('[data-log]').forEach(row => { const show = () => modal('日志详情', `<pre>${e(JSON.stringify(rows[Number(row.dataset.log)], null, 2))}</pre>`); row.onclick = show; row.onkeydown = ev => { if (ev.key === 'Enter')
        show(); }; });
}
async function exportLogs() {
    try {
        if (native && !demo) {
            const v = await window.__TAURI__.core.invoke('export_logs');
            toast(`日志已写入 ${v.path}`);
            return;
        }
        const logs = filterLogs(state.logs, state.filter);
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
        const p = progress(t.received_bytes, t.total_bytes);
        const done = t.status === 'completed';
        const active = ['running', 'queued'].includes(t.status);
        return `<article class="card transfer-live"><div class="file-mark">${t.direction === 'send' ? '↗' : '↙'}</div><div class="file-main"><h3>${e(t.filename)} <span class="tag">${e(statusNames[t.status] ?? t.status)}</span></h3><div class="subtle" style="font-size:11px">${e(t.direction === 'send' ? '本机 → 对端' : t.direction === 'staging' ? '本机上传暂存' : t.target === 'local' ? '本机下载' : '对端 → 本机')} · ${e(stages[t.stage] ?? t.stage)}</div><div class="progress-track"><div class="progress-fill ${p === null ? 'unknown' : ''}" style="width:${done ? 100 : p ?? 20}%"></div></div><div class="file-info"><span>${bytes(t.received_bytes)} / ${t.total_bytes === null ? '大小未知' : bytes(t.total_bytes)}</span><span>${done ? 'SHA-256 已校验' : p === null ? '等待数据' : `${p.toFixed(1)}%`}</span></div>${t.error ? `<p class="warning-copy break">${e(t.error)}</p>` : ''}</div><div class="actions">${done && t.direction !== 'send' ? `<button class="btn small" data-save="${e(t.id)}">${native ? '文件位置' : '保存文件'}</button>` : ''}<button class="btn small" data-transfer="${e(t.id)}">详情</button><button class="btn small ${active ? '' : 'quiet'}" data-cancel="${e(t.id)}">${active ? '取消' : '移除'}</button></div></article>`;
    }).join('') : '<div class="card empty-state">暂无文件任务。新建下载或选择一个文件开始传输。</div>';
    document.querySelectorAll('[data-transfer]').forEach(b => b.onclick = () => { const t = state.transfers.find(t => t.id === b.dataset.transfer); modal('传输详情', `<pre>${e(JSON.stringify(t, null, 2))}</pre>`); });
    document.querySelectorAll('[data-cancel]').forEach(b => b.onclick = () => void api('DELETE', `/v1/downloads/${b.dataset.cancel}`).then(() => { toast('操作已提交'); void refresh(); }).catch(fail));
    document.querySelectorAll('[data-save]').forEach(b => b.onclick = () => { const t = state.transfers.find(t => t.id === b.dataset.save); void saveContent(t.id, t.filename).then(message => { if (message)
        modal('文件位置', `<pre>${e(message)}</pre>`); }).catch(fail); });
}
function downloadDialog() {
    modal('新建文件下载', `<form id="download-form"><div class="form-row"><label class="form-label">执行位置<select name="target"><option value="peer">对端节点</option><option value="local">本机节点</option></select></label><label class="form-label">执行器<select name="executor"><option value="browser">浏览器扩展</option><option value="http">普通 HTTP</option></select></label></div><label class="form-label">最终下载 URL<input name="url" type="url" required placeholder="https://example.com/files/report.zip"></label><label class="form-label">文件名<input name="filename" required value="report.zip" maxlength="120"></label><p class="form-hint">目标来源必须被执行节点允许；浏览器执行器还需扩展网站授权。Alpha 不跟随重定向。</p><button class="btn primary" type="submit">创建下载</button></form>`);
    el('download-form').onsubmit = event => { event.preventDefault(); const form = el('download-form'); const data = Object.fromEntries(new FormData(form)); const button = form.querySelector('button'); button.disabled = true; void api('POST', '/v1/downloads', data).then(() => { el('dialog').close(); toast('下载任务已创建'); void refresh(); }).catch(fail).finally(() => button.disabled = false); };
}
function pullDialog() { modal('从对端共享目录拉取文件', `<form id="pull-form"><label class="form-label">共享目录中的文件名<input name="filename" placeholder="report.zip" required maxlength="120"></label><p class="form-hint">Alpha 只允许共享根目录下的普通文件名，不接收任意磁盘路径。</p><button class="btn primary" type="submit">拉取文件</button></form>`); el('pull-form').onsubmit = event => { event.preventDefault(); const data = Object.fromEntries(new FormData(el('pull-form'))); void api('POST', '/v1/files/pull', data).then(() => { el('dialog').close(); void refresh(); }).catch(fail); }; }
function renderSettings() {
    const c = state.settings;
    if (!c) {
        el('page-content').innerHTML = head('设置', '只配置需要的选项。') + `<section class="card empty-state">尚未读取到服务配置。<br><button id="settings-login" class="btn connection-entry">连接本机服务</button></section>`;
        el('settings-login').onclick = () => { if (native)
            void refresh();
        else
            el('connect-dialog').showModal(); };
        return;
    }
    el('page-content').innerHTML = head('设置', '两端使用相同配置结构；串口、目标权限和配对分别管理。') + `<div class="settings-grid-live"><form id="settings-form"><section class="card settings-box"><h2>节点与串口</h2><label class="form-label">节点名称<input name="node_name" value="${e(c.node_name)}" maxlength="80" required></label><label class="form-label">串口<input name="serial_port" list="port-options" value="${e(c.serial_port)}" placeholder="COM5 或 /dev/cu.usbserial-…"><datalist id="port-options">${state.ports.map(p => `<option value="${e(p.name)}">${e(p.product ?? '')}</option>`).join('')}</datalist></label><div class="form-row"><label class="form-label">波特率<select name="baud_rate">${[...new Set([9600, 57600, 115200, 230400, 460800, 921600, 1000000, 2000000, 3000000, c.baud_rate])].sort((a, b) => a - b).map(n => `<option value="${n}" ${n === c.baud_rate ? 'selected' : ''}>${n}</option>`).join('')}</select></label><label class="form-label">本地 HTTP 端口<input name="http_port" type="number" min="1" max="65535" value="${c.http_port}" required></label></div><label class="form-label"><input name="auto_reconnect" type="checkbox" style="display:inline;width:auto;margin-right:7px" ${c.auto_reconnect ? 'checked' : ''}>断开后自动重连</label><p class="form-hint">更改串口会重建链路。更改 HTTP 端口需要重启程序。两端波特率必须一致；初次验证建议 115200。</p></section><section class="card settings-box"><h2>允许执行的 HTTP 请求</h2><label class="form-label">来源白名单，每行一个<textarea name="allowed_origins" rows="4" placeholder="https://example.com">${e(c.allowed_origins.join('\n'))}</textarea></label><label class="form-label">允许的方法<input name="allowed_methods" value="${e(c.allowed_methods.join(', '))}" required></label><p class="form-hint">精确到协议、主机与端口，不填写路径或通配符。留空表示全部拒绝。POST 等写入操作需明确启用。</p></section><div class="settings-footer"><button class="btn primary" type="submit">保存设置</button><span class="form-hint">仅修改本机配置，不替对端保存。</span></div></form><div><section class="card settings-box"><h2>浏览器扩展</h2><div id="settings-browser">${badge(Boolean(state.snapshot?.browser.connected))}</div><p class="form-hint">扩展主动连接本机；配对凭据不会通过串口发送。</p><div id="pair-result"></div><div class="quick-row" style="padding:10px 0 0"><button class="btn primary" id="new-pair">生成配对码</button><button class="btn danger" id="revoke-pair">解除配对</button></div><p class="form-hint">在扩展弹窗中输入端口与配对码。代码有效期 3 分钟，最多尝试 5 次。</p></section><section class="card settings-box"><h2>文件与日志</h2><label class="form-label">下载目录<div class="mono break form-hint">${e(c.download_dir)}</div></label><label class="form-label">共享目录<div class="mono break form-hint">${e(c.shared_dir)}</div></label><p class="form-hint">单文件上限 ${bytes(c.max_file_bytes)} · 目录配额 ${bytes(c.max_spool_bytes)}<br>日志保留最近 2000 条，可在日志页导出。目录路径请在停止程序后修改 TOML 配置。</p></section><section class="card settings-box"><h2>Alpha 边界</h2><p class="form-hint">没有硬件时不显示虚构连接。浏览器关闭后相关任务失败，不偷偷改为普通 HTTP。当前没有开机自启、断点续传或多浏览器配置。</p>${!native ? '<button class="btn" id="settings-token">更换 API 凭据</button>' : ''}</section></div></div>`;
    el('settings-form').onsubmit = event => { event.preventDefault(); const form = el('settings-form'); const f = new FormData(form); const patch = { node_name: String(f.get('node_name')), serial_port: String(f.get('serial_port')), baud_rate: Number(f.get('baud_rate')), http_port: Number(f.get('http_port')), auto_reconnect: f.get('auto_reconnect') === 'on', allowed_origins: String(f.get('allowed_origins')).split(/\r?\n/).map(s => s.trim()).filter(Boolean), allowed_methods: String(f.get('allowed_methods')).split(/[\s,]+/).map(s => s.toUpperCase()).filter(Boolean) }; const button = form.querySelector('button[type=submit]'); button.disabled = true; void api('PUT', '/v1/settings', patch).then(r => { state.settings = r.settings; toast(r.restart_required ? '配置已保存。HTTP 端口变更需重启后生效。' : '设置已保存'); void refresh(); }).catch(fail).finally(() => button.disabled = false); };
    el('new-pair').onclick = () => void api('POST', '/v1/extension/pairing', {}).then(r => { el('pair-result').innerHTML = `<div class="pair-code">${e(r.code)}</div><p class="form-hint">请在 ${r.expires_in_seconds} 秒内输入扩展。不要把此配对码发给其他人。</p>`; }).catch(fail);
    el('revoke-pair').onclick = () => void api('DELETE', '/v1/extension/pairing').then(() => { el('pair-result').innerHTML = ''; toast('本机扩展配对已解除'); void refresh(); }).catch(fail);
    document.getElementById('settings-token')?.addEventListener('click', () => el('connect-dialog').showModal());
}
let forwardDraft = null;
function renderTunnels() {
    el('page-content').innerHTML = head('TCP 转发', '本机监听端口 → USB 串口 → 对端连接目标地址。字节原样透传，可承载 VLESS、TLS、SSH 等任意 TCP 协议。') + `<section class="card settings-box"><div class="section-head"><div><h2>转发规则</h2><div class="section-desc">「对端目标地址」是对端电脑视角下的地址，例如对端本机 Xray 的 127.0.0.1:443。</div></div><button class="btn" id="add-forward">添加规则</button></div><form id="forward-form"><div id="forward-rows"></div><div class="settings-footer"><button class="btn primary" type="submit">保存规则</button><span class="form-hint">保存后监听立即重建，已建立的连接不受影响。监听地址建议使用 127.0.0.1。</span></div></form></section><section class="card" style="margin-top:18px"><div class="section-head"><div><h2>活动连接</h2><div class="section-desc" id="stream-summary"></div></div></div><div class="table-wrap"><table class="live-table"><thead><tr><th>方向</th><th>本机监听 / 来源</th><th>目标</th><th>已发送</th><th>已接收</th><th>开始时间</th></tr></thead><tbody id="stream-rows"></tbody></table></div></section>`;
    if (!forwardDraft)
        forwardDraft = (state.settings?.forwards ?? state.tunnels?.forwards.map(f => ({ listen: f.listen, target: f.target })) ?? []).map(f => ({ ...f }));
    el('add-forward').onclick = () => { syncDraft(); forwardDraft.push({ listen: '127.0.0.1:', target: '127.0.0.1:' }); updateForwardRows(); };
    el('forward-form').onsubmit = event => { event.preventDefault(); syncDraft(); const forwards = forwardDraft.filter(f => f.listen.trim() || f.target.trim()).map(f => ({ listen: f.listen.trim(), target: f.target.trim() })); const button = el('forward-form').querySelector('button[type=submit]'); button.disabled = true; void api('PUT', '/v1/settings', { forwards }).then(r => { state.settings = r.settings; forwardDraft = r.settings.forwards.map(f => ({ ...f })); toast('转发规则已保存'); void refresh(); }).catch(fail).finally(() => button.disabled = false); };
    updateForwardRows();
    updateTunnels();
}
function syncDraft() { document.querySelectorAll('[data-forward]').forEach(row => { const i = Number(row.dataset.forward); forwardDraft[i] = { listen: row.querySelector('[name=listen]').value, target: row.querySelector('[name=target]').value }; }); }
function forwardStatus(f) {
    const s = state.tunnels?.forwards.find(s => s.listen === f.listen && s.target === f.target);
    const live = !state.snapshot || !state.tunnels ? badge(false, '状态未知') : s ? (s.listening ? badge(true, '监听中') : badge(false, s.error ? '监听失败' : '未监听')) : badge(false, '未保存');
    return `${live}${s?.error ? `<p class="warning-copy break">${e(s.error)}</p>` : ''}`;
}
function updateForwardRows() {
    el('forward-rows').innerHTML = forwardDraft.length ? forwardDraft.map((f, i) => `<div class="form-row" data-forward="${i}"><label class="form-label">本机监听地址<input name="listen" value="${e(f.listen)}" placeholder="127.0.0.1:10443" required></label><label class="form-label">对端目标地址<input name="target" value="${e(f.target)}" placeholder="127.0.0.1:443" required></label><div class="form-label"><span data-forward-status>${forwardStatus(f)}</span><button class="btn small quiet" type="button" data-remove-forward="${i}">删除</button></div></div>`).join('') : '<div class="empty-state">还没有转发规则。点击「添加规则」，例如 127.0.0.1:10443 → 127.0.0.1:443。</div>';
    document.querySelectorAll('[data-remove-forward]').forEach(b => b.onclick = () => { syncDraft(); forwardDraft.splice(Number(b.dataset.removeForward), 1); updateForwardRows(); });
}
function updateTunnels() {
    // 只刷新状态，不替换输入框；编辑规则时也要及时撤销离线的监听标识。
    document.querySelectorAll('[data-forward]').forEach(row => {
        row.querySelector('[data-forward-status]').innerHTML = forwardStatus({ listen: row.querySelector('[name=listen]').value, target: row.querySelector('[name=target]').value });
    });
    const t = state.tunnels;
    const rows = t?.streams ?? [];
    el('stream-summary').textContent = t ? `${rows.length} 个活动连接 · 累计 ${t.total_streams} 个` : '尚未读取到转发状态';
    el('stream-rows').innerHTML = rows.length ? rows.map(s => `<tr><td>${s.direction === 'outbound' ? '本机 → 对端' : '对端 → 本机'}</td><td class="mono">${e(s.direction === 'outbound' ? `${s.listen} ← ${s.client}` : '对端发起')}</td><td class="mono">${e(s.target)}</td><td>${bytes(s.bytes_sent)}</td><td>${bytes(s.bytes_received)}</td><td class="mono subtle">${time(s.started_at)}</td></tr>`).join('') : '<tr><td colspan="6"><div class="empty-state">没有活动连接</div></td></tr>';
}
async function navigate(page) {
    if (!labels[page])
        return;
    state.page = page;
    el('page-title').textContent = labels[page];
    document.querySelectorAll('[data-page]').forEach(b => b.classList.toggle('active', b.dataset.page === page));
    if (page === 'tunnels') {
        forwardDraft = null;
        if (hasCredentials())
            try {
                state.settings = await api('GET', '/v1/settings');
            }
            catch (err) {
                fail(err);
            }
    }
    if (page === 'settings' && hasCredentials()) {
        try {
            const [c, p] = await Promise.all([api('GET', '/v1/settings'), api('GET', '/v1/ports')]);
            state.settings = c;
            state.ports = p.ports;
        }
        catch (err) {
            fail(err);
        }
    }
    renderPage();
}
function renderPage() { if (state.page === 'overview')
    renderOverview();
else if (state.page === 'logs')
    renderLogs();
else if (state.page === 'transfers')
    renderTransfers();
else if (state.page === 'tunnels')
    renderTunnels();
else
    renderSettings(); }
let loading = false;
async function refresh() {
    if (loading || !hasCredentials())
        return;
    loading = true;
    try {
        const [s, l, t, tn] = await Promise.all([api('GET', '/v1/health'), api('GET', '/v1/logs'), api('GET', '/v1/downloads'), api('GET', '/v1/tunnels').catch(() => null)]);
        state.snapshot = s;
        state.tunnels = tn;
        if (!state.paused)
            state.logs = l.logs;
        state.transfers = t.transfers;
        state.error = '';
    }
    catch (err) {
        state.snapshot = null;
        state.tunnels = null;
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
        else if (state.page === 'tunnels') {
            if (document.getElementById('stream-rows'))
                updateTunnels();
        }
        else if (document.getElementById('settings-browser'))
            el('settings-browser').innerHTML = badge(Boolean(state.snapshot?.browser.connected));
    }
}
async function ping(target = 'peer') { try {
    const before = performance.now();
    const r = await api('POST', '/v1/rpc', { target, method: 'system.ping', params: {} });
    modal(`${target === 'peer' ? '对端' : '本机'}测试结果`, `${demo ? '<p class="form-hint">模拟请求，不代表真实串口往返时间。</p>' : `<p class="form-hint">本次调用耗时 ${Math.round(performance.now() - before)} ms</p>`}<pre>${e(JSON.stringify(r, null, 2))}</pre>`);
    void refresh();
}
catch (err) {
    fail(err);
} }
el('close-dialog').onclick = () => el('dialog').close();
el('close-connect').onclick = () => el('connect-dialog').close();
el('refresh').onclick = () => void refresh();
el('ping').onclick = () => void ping();
for (const nav of document.querySelectorAll('[data-page]'))
    nav.onclick = () => void navigate(nav.dataset.page);
el('connect-form').onsubmit = event => { event.preventDefault(); try {
    configure(Number(el('api-port').value), el('api-token').value.trim());
    el('api-token').value = '';
    el('connect-dialog').close();
    void refresh().then(() => { if (state.page === 'settings')
        void navigate('settings'); });
}
catch (err) {
    fail(err);
} };
el('push-picker').onchange = () => { const input = el('push-picker'); const file = input.files?.[0]; if (!file)
    return; toast(`正在准备 ${file.name}`); void pushFile(file, n => toast(`上传至本机节点：${bytes(n)} / ${bytes(file.size)}`)).then(() => { toast('文件已进入对端传输队列'); void navigate('transfers'); void refresh(); }).catch(fail).finally(() => input.value = ''); };
el('run-mode').textContent = demo ? 'DEMO' : native ? 'NATIVE' : 'HTTP';
el('demo-banner').classList.toggle('hidden', !demo);
renderPage();
topStatus();
if (!hasCredentials())
    el('connect-dialog').showModal();
else
    void refresh();
setInterval(() => void refresh(), 1500);
