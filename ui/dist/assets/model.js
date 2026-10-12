export function deriveStatus(snapshot) {
    if (!snapshot || typeof snapshot !== 'object')
        return { core: false, peer: false, localBrowser: false, remoteBrowser: false };
    const s = snapshot;
    return { core: true, peer: s.peer?.connected === true, localBrowser: s.browser?.connected === true, remoteBrowser: s.peer?.connected === true && s.peer?.browser === true };
}
export function filterLogs(logs, f) {
    return logs.filter(x => (!f.level || f.level === 'all' || x.level === f.level) && (!f.component || f.component === 'all' || x.component === f.component) && `${x.message} ${x.id ?? ''} ${x.component}`.toLowerCase().includes(f.query.toLowerCase()));
}
export function progress(received, total) { return total && total > 0 ? Math.min(100, Math.max(0, 100 * received / total)) : null; }
export function escapeHtml(x) { return String(x ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }
