/** At most one unacknowledged binary block per download. */
export class AckWindow {
    pending = new Map();
    wait(id, offset, signal, timeout = 15000) {
        if (this.pending.has(id))
            return Promise.reject(Error('WINDOW_FULL'));
        signal.throwIfAborted();
        return new Promise((resolve, reject) => {
            const abort = () => this.reject(id, Error('CANCELLED'));
            const timer = setTimeout(() => this.reject(id, Error('ACK_TIMEOUT')), timeout);
            const clear = () => { clearTimeout(timer); signal.removeEventListener('abort', abort); };
            this.pending.set(id, { offset, resolve, reject, clear });
            signal.addEventListener('abort', abort, { once: true });
        });
    }
    ack(id, offset) {
        const p = this.pending.get(id);
        if (!p)
            return false;
        if (p.offset !== offset) {
            this.reject(id, Error('ACK_OFFSET_MISMATCH'));
            return false;
        }
        this.pending.delete(id);
        p.clear();
        p.resolve();
        return true;
    }
    reject(id, error) { const p = this.pending.get(id); if (p) {
        this.pending.delete(id);
        p.clear();
        p.reject(error);
    } }
    reset() { for (const id of this.pending.keys())
        this.reject(id, Error('BROWSER_DISCONNECTED')); }
}
