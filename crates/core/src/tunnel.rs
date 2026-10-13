//! Transparent TCP forwarding over the serial link.
//!
//! The listening node (A) accepts a local TCP connection and asks the peer (B)
//! to connect to the configured `target`. Bytes are relayed unchanged in both
//! directions; SerialBridge never inspects the application protocol, so VLESS,
//! TLS, SSH, etc. pass through as-is. Each stream has its own byte credit so a
//! busy stream cannot starve the others or grow peer memory without bound.
use crate::{
    error::{BridgeError, Result},
    node::Node,
    protocol::{self, Packet},
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Notify, Semaphore},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Bytes a sender may have unacknowledged per stream and direction.
pub const WINDOW: usize = 256 * 1024;
/// Payload of one STREAM_DATA frame.
pub const CHUNK: usize = 16 * 1024;
/// Simultaneous streams per node (both directions combined).
pub const MAX_STREAMS: usize = 256;
pub const MAX_FORWARDS: usize = 32;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    /// Local listen address on this node, e.g. `127.0.0.1:10443`.
    pub listen: String,
    /// Address the peer connects to, as seen from the peer, e.g. `127.0.0.1:443`.
    pub target: String,
}

/// Splits `host:port` / `[v6]:port`; the host is resolved by the peer.
pub fn split_target(target: &str) -> Result<(String, u16)> {
    let bad = || {
        BridgeError::invalid(format!(
            "Invalid target address: {target} (use host:port or [IPv6]:port)"
        ))
    };
    let (host, port) = if let Some(rest) = target.strip_prefix('[') {
        let (host, port) = rest.split_once("]:").ok_or_else(bad)?;
        host.parse::<std::net::Ipv6Addr>().map_err(|_| bad())?;
        (host, port)
    } else {
        let (host, port) = target.rsplit_once(':').ok_or_else(bad)?;
        if host.contains(':') {
            return Err(bad());
        }
        (host, port)
    };
    let port: u16 = port.parse().map_err(|_| bad())?;
    if host.is_empty()
        || host.len() > 253
        || port == 0
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/\\@".contains(c))
    {
        return Err(bad());
    }
    Ok((host.to_string(), port))
}

pub fn validate_forwards(forwards: &[Forward]) -> Result<()> {
    if forwards.len() > MAX_FORWARDS {
        return Err(BridgeError::invalid(format!(
            "At most {MAX_FORWARDS} forwards"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for f in forwards {
        let addr: std::net::SocketAddr = f.listen.parse().map_err(|_| {
            BridgeError::invalid(format!(
                "Invalid listen address: {} (use ip:port, e.g. 127.0.0.1:10443)",
                f.listen
            ))
        })?;
        if addr.port() == 0 {
            return Err(BridgeError::invalid("Listen port must not be 0"));
        }
        if !seen.insert(addr) {
            return Err(BridgeError::invalid(format!(
                "Duplicate listen address: {}",
                f.listen
            )));
        }
        split_target(&f.target)?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum StreamEvent {
    Ready,
    Data(Bytes),
    Fin,
    Reset(BridgeError),
}

pub struct StreamEntry {
    pub epoch: Uuid,
    pub events: mpsc::UnboundedSender<StreamEvent>,
    pub credit: Arc<Semaphore>,
    pub buffered: Arc<AtomicUsize>,
    pub info: Arc<StreamInfo>,
}

pub struct StreamInfo {
    pub id: Uuid,
    /// `outbound`: accepted locally and sent to the peer; `inbound`: opened by the peer.
    pub direction: &'static str,
    pub listen: Option<String>,
    pub target: String,
    pub client: Option<String>,
    pub started_at: u64,
    pub sent: AtomicU64,
    pub received: AtomicU64,
}
impl StreamInfo {
    fn json(&self) -> Value {
        json!({"id":self.id,"direction":self.direction,"listen":self.listen,"target":self.target,"client":self.client,"started_at":self.started_at,"bytes_sent":self.sent.load(Ordering::Relaxed),"bytes_received":self.received.load(Ordering::Relaxed)})
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ListenerStatus {
    pub listen: String,
    pub target: String,
    pub listening: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Tunnels {
    pub streams: Mutex<HashMap<Uuid, StreamEntry>>,
    pub listeners: Mutex<Vec<ListenerStatus>>,
    pub changed: Notify,
    pub total: AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Node {
    pub fn tunnel_snapshot(&self) -> Value {
        let mut streams: Vec<_> = self
            .tunnels
            .streams
            .lock()
            .unwrap()
            .values()
            .map(|s| s.info.clone())
            .collect();
        streams.sort_by_key(|s| s.started_at);
        json!({
            "forwards": self.tunnels.listeners.lock().unwrap().clone(),
            "streams": streams.iter().map(|s| s.json()).collect::<Vec<_>>(),
            "total_streams": self.tunnels.total.load(Ordering::Relaxed),
        })
    }

    /// Resets every stream that belongs to a lost serial session.
    pub(crate) fn tunnels_lost(&self, epoch: Uuid) {
        let lost: Vec<_> = {
            let mut map = self.tunnels.streams.lock().unwrap();
            let ids: Vec<_> = map
                .iter()
                .filter(|(_, s)| s.epoch == epoch)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter().filter_map(|id| map.remove(&id)).collect()
        };
        for s in lost {
            s.credit.close();
            let _ = s.events.send(StreamEvent::Reset(BridgeError::new(
                "PEER_DISCONNECTED",
                "Serial link lost; forwarded connection closed",
            )));
        }
    }

    /// Routes a STREAM_* frame from the session read loop. Never blocks.
    pub(crate) fn tunnel_packet(
        self: &Arc<Self>,
        epoch: Uuid,
        ctl: &mpsc::Sender<Packet>,
        packet: Packet,
    ) -> Result<()> {
        if packet.kind == protocol::STREAM_OPEN {
            let node = self.clone();
            tokio::spawn(async move { node.inbound(epoch, packet).await });
            return Ok(());
        }
        let mut map = self.tunnels.streams.lock().unwrap();
        let Some(entry) = map.get(&packet.id).filter(|s| s.epoch == epoch) else {
            // Late frames for a stream that already closed locally are harmless.
            return Ok(());
        };
        match packet.kind {
            protocol::STREAM_READY => {
                let _ = entry.events.send(StreamEvent::Ready);
            }
            protocol::STREAM_DATA => {
                let n = packet.data.len();
                if entry.buffered.fetch_add(n, Ordering::AcqRel) + n > WINDOW {
                    // Only this stream is broken; the link and other streams continue.
                    let error = BridgeError::new("FLOW_CONTROL", "Peer exceeded stream window");
                    if let Some(entry) = map.remove(&packet.id) {
                        entry.credit.close();
                        let _ = entry.events.send(StreamEvent::Reset(error.clone()));
                    }
                    send_reset(ctl, packet.id, &error);
                    return Ok(());
                }
                let _ = entry.events.send(StreamEvent::Data(packet.data));
            }
            protocol::STREAM_ACK => {
                let n = packet.meta["bytes"].as_u64().unwrap_or(0) as usize;
                let room = WINDOW.saturating_sub(entry.credit.available_permits());
                entry.credit.add_permits(n.min(room));
            }
            protocol::STREAM_FIN => {
                let _ = entry.events.send(StreamEvent::Fin);
            }
            protocol::STREAM_RESET => {
                let error = serde_json::from_value(packet.meta["error"].clone())
                    .unwrap_or_else(|_| BridgeError::new("STREAM_RESET", "Peer reset the stream"));
                if let Some(entry) = map.remove(&packet.id) {
                    entry.credit.close();
                    let _ = entry.events.send(StreamEvent::Reset(error));
                }
            }
            _ => return Err(BridgeError::invalid("Invalid stream frame")),
        }
        Ok(())
    }

    /// Registers a stream on the current ready session.
    fn register_stream(
        self: &Arc<Self>,
        id: Uuid,
        info: Arc<StreamInfo>,
        require_epoch: Option<Uuid>,
    ) -> Result<StreamHandle> {
        let peer = self.peer.lock().unwrap();
        let link = peer
            .as_ref()
            .filter(|p| p.ready && require_epoch.is_none_or(|e| e == p.epoch))
            .ok_or_else(|| BridgeError::new("PEER_DISCONNECTED", "Serial peer is not ready"))?;
        if require_epoch.is_none() && link.remote["tunnel"] != 1 {
            return Err(BridgeError::new(
                "PEER_UNSUPPORTED",
                "Peer SerialBridge does not support TCP forwarding; upgrade both nodes",
            ));
        }
        let mut map = self.tunnels.streams.lock().unwrap();
        if map.len() >= MAX_STREAMS {
            return Err(BridgeError::new("BUSY", "Too many forwarded connections"));
        }
        if map.contains_key(&id) {
            return Err(BridgeError::invalid("Duplicate stream ID"));
        }
        let (events_tx, events) = mpsc::unbounded_channel();
        let credit = Arc::new(Semaphore::new(WINDOW));
        let buffered = Arc::new(AtomicUsize::new(0));
        map.insert(
            id,
            StreamEntry {
                epoch: link.epoch,
                events: events_tx,
                credit: credit.clone(),
                buffered: buffered.clone(),
                info: info.clone(),
            },
        );
        self.tunnels.total.fetch_add(1, Ordering::Relaxed);
        Ok(StreamHandle {
            node: self.clone(),
            id,
            tx: link.tx.clone(),
            ctl: link.ctl.clone(),
            events,
            credit,
            buffered,
            info,
            done: false,
        })
    }

    /// Peer side: connect to the requested target and relay.
    async fn inbound(self: Arc<Self>, epoch: Uuid, packet: Packet) {
        let id = packet.id;
        let target = packet.meta["target"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let info = Arc::new(StreamInfo {
            id,
            direction: "inbound",
            listen: None,
            target: target.clone(),
            client: None,
            started_at: now_ms(),
            sent: AtomicU64::new(0),
            received: AtomicU64::new(0),
        });
        let reject = |e: BridgeError| {
            if let Some(link) = self
                .peer
                .lock()
                .unwrap()
                .as_ref()
                .filter(|p| p.epoch == epoch)
            {
                send_reset(&link.ctl, id, &e);
            }
            self.log(
                "warn",
                "tunnel",
                format!("Rejected stream to {target}: {}", e.message),
                id,
            );
        };
        let (host, port) = match split_target(&target) {
            Ok(v) => v,
            Err(e) => return reject(e),
        };
        let mut handle = match self.register_stream(id, info, Some(epoch)) {
            Ok(h) => h,
            Err(e) => return reject(e),
        };
        let connect =
            tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port)));
        let socket = tokio::select! {
            r = connect => match r {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return handle.fail(BridgeError::new("CONNECT_FAILED", format!("Connect {target} failed: {e}"))),
                Err(_) => return handle.fail(BridgeError::new("TIMEOUT", format!("Connect {target} timed out"))),
            },
            ev = handle.events.recv() => match ev {
                Some(StreamEvent::Reset(e)) => {
                    // Peer reset or link lost before we connected; the peer already knows.
                    handle.done = true;
                    self.log("info", "tunnel", format!("Stream cancelled before connect: {}: {}", e.code, e.message), id);
                    return;
                }
                None => { handle.done = true; return; }
                Some(_) => return handle.fail(BridgeError::invalid("Stream frame before ready")),
            }
        };
        let _ = socket.set_nodelay(true);
        if handle
            .ctl
            .send(Packet::new(
                protocol::STREAM_READY,
                id,
                json!({}),
                Bytes::new(),
            ))
            .await
            .is_err()
        {
            handle.done = true;
            return;
        }
        self.log(
            "info",
            "tunnel",
            format!("Peer stream connected to {target}"),
            id,
        );
        handle.run(socket).await;
    }

    /// Local side: a client connected to one of our forward listeners.
    async fn outbound(
        self: Arc<Self>,
        socket: TcpStream,
        forward: Forward,
        client: std::net::SocketAddr,
    ) {
        let id = Uuid::new_v4();
        let info = Arc::new(StreamInfo {
            id,
            direction: "outbound",
            listen: Some(forward.listen.clone()),
            target: forward.target.clone(),
            client: Some(client.to_string()),
            started_at: now_ms(),
            sent: AtomicU64::new(0),
            received: AtomicU64::new(0),
        });
        let mut handle = match self.register_stream(id, info, None) {
            Ok(h) => h,
            Err(e) => {
                self.log(
                    "warn",
                    "tunnel",
                    format!(
                        "{} → {} refused: {}",
                        forward.listen, forward.target, e.message
                    ),
                    id,
                );
                return;
            }
        };
        let open = Packet::new(
            protocol::STREAM_OPEN,
            id,
            json!({"target":forward.target}),
            Bytes::new(),
        );
        if handle.ctl.send(open).await.is_err() {
            return handle.fail(BridgeError::new(
                "PEER_DISCONNECTED",
                "Serial write queue closed",
            ));
        }
        match tokio::time::timeout(OPEN_TIMEOUT, handle.events.recv()).await {
            Ok(Some(StreamEvent::Ready)) => {}
            Ok(Some(StreamEvent::Reset(e))) => {
                handle.done = true;
                self.log(
                    "warn",
                    "tunnel",
                    format!(
                        "{} → {} failed: {}",
                        forward.listen, forward.target, e.message
                    ),
                    id,
                );
                return;
            }
            Ok(None) => {
                handle.done = true;
                return;
            }
            Ok(Some(_)) => return handle.fail(BridgeError::invalid("Stream data before ready")),
            Err(_) => {
                return handle.fail(BridgeError::new(
                    "TIMEOUT",
                    "Peer did not answer the stream open",
                ))
            }
        }
        let _ = socket.set_nodelay(true);
        self.log(
            "info",
            "tunnel",
            format!("{} → {} opened ({client})", forward.listen, forward.target),
            id,
        );
        handle.run(socket).await;
    }
}

fn reset_packet(id: Uuid, error: &BridgeError) -> Packet {
    Packet::new(
        protocol::STREAM_RESET,
        id,
        json!({"error":error}),
        Bytes::new(),
    )
}

/// Sends RESET without blocking the caller and without dropping it when the
/// control queue is momentarily full; a lost RESET would leak the peer's stream.
fn send_reset(ctl: &mpsc::Sender<Packet>, id: Uuid, error: &BridgeError) {
    if let Err(mpsc::error::TrySendError::Full(packet)) = ctl.try_send(reset_packet(id, error)) {
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let ctl = ctl.clone();
            rt.spawn(async move {
                let _ = ctl.send(packet).await;
            });
        }
    }
}

/// Marks an end that the peer (or link loss) already reported, keeping its code.
const REMOTE_END: &str = "REMOTE_END";

pub struct StreamHandle {
    node: Arc<Node>,
    id: Uuid,
    tx: mpsc::Sender<Packet>,
    ctl: mpsc::Sender<Packet>,
    events: mpsc::UnboundedReceiver<StreamEvent>,
    credit: Arc<Semaphore>,
    buffered: Arc<AtomicUsize>,
    info: Arc<StreamInfo>,
    /// Set when the peer already knows the stream ended (clean close, remote reset, link lost).
    done: bool,
}

impl StreamHandle {
    fn fail(mut self, error: BridgeError) {
        self.node.log(
            "warn",
            "tunnel",
            format!("Stream to {} failed: {}", self.info.target, error.message),
            self.id,
        );
        if self
            .node
            .tunnels
            .streams
            .lock()
            .unwrap()
            .remove(&self.id)
            .is_some()
        {
            send_reset(&self.ctl, self.id, &error);
        }
        self.done = true;
    }

    /// Relays until both directions finished (clean) or either side failed.
    async fn run(mut self, socket: TcpStream) {
        let (mut reader, mut writer) = socket.into_split();
        let (id, tx, ctl, credit, buffered, info) = (
            self.id,
            self.tx.clone(),
            self.ctl.clone(),
            self.credit.clone(),
            self.buffered.clone(),
            self.info.clone(),
        );
        let events = &mut self.events;
        let upload = async {
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| BridgeError::new("IO_ERROR", e.to_string()))?;
                if n == 0 {
                    tx.send(Packet::new(
                        protocol::STREAM_FIN,
                        id,
                        json!({}),
                        Bytes::new(),
                    ))
                    .await
                    .map_err(|_| {
                        BridgeError::new("PEER_DISCONNECTED", "Serial write queue closed")
                    })?;
                    return Ok::<_, BridgeError>(());
                }
                credit
                    .acquire_many(n as u32)
                    .await
                    .map_err(|_| BridgeError::new("PEER_DISCONNECTED", "Stream closed"))?
                    .forget();
                tx.send(Packet::new(
                    protocol::STREAM_DATA,
                    id,
                    json!({}),
                    Bytes::copy_from_slice(&buf[..n]),
                ))
                .await
                .map_err(|_| BridgeError::new("PEER_DISCONNECTED", "Serial write queue closed"))?;
                info.sent.fetch_add(n as u64, Ordering::Relaxed);
            }
        };
        let download = async {
            loop {
                match events.recv().await {
                    Some(StreamEvent::Data(data)) => {
                        let n = data.len();
                        writer
                            .write_all(&data)
                            .await
                            .map_err(|e| BridgeError::new("IO_ERROR", e.to_string()))?;
                        buffered.fetch_sub(n, Ordering::AcqRel);
                        info.received.fetch_add(n as u64, Ordering::Relaxed);
                        ctl.send(Packet::new(
                            protocol::STREAM_ACK,
                            id,
                            json!({"bytes":n}),
                            Bytes::new(),
                        ))
                        .await
                        .map_err(|_| {
                            BridgeError::new("PEER_DISCONNECTED", "Serial write queue closed")
                        })?;
                    }
                    Some(StreamEvent::Fin) => {
                        let _ = writer.shutdown().await;
                        return Ok(());
                    }
                    Some(StreamEvent::Reset(e)) => {
                        return Err(BridgeError::new(
                            REMOTE_END,
                            format!("{}: {}", e.code, e.message),
                        ))
                    }
                    Some(StreamEvent::Ready) => {
                        return Err(BridgeError::invalid("Duplicate stream ready"))
                    }
                    None => return Err(BridgeError::new(REMOTE_END, "Stream closed")),
                }
            }
        };
        let result = tokio::try_join!(upload, download);
        let summary = format!(
            "Stream to {} closed (sent {} B, received {} B)",
            self.info.target,
            self.info.sent.load(Ordering::Relaxed),
            self.info.received.load(Ordering::Relaxed)
        );
        match result {
            Ok(_) => {
                self.done = true;
                self.node.tunnels.streams.lock().unwrap().remove(&self.id);
                self.node.log("info", "tunnel", summary, self.id);
            }
            Err(e) if e.code == REMOTE_END => {
                self.done = true;
                self.node.tunnels.streams.lock().unwrap().remove(&self.id);
                self.node.log(
                    "info",
                    "tunnel",
                    format!("{summary}: {}", e.message),
                    self.id,
                );
            }
            Err(e) => self.fail(e),
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Covers task cancellation/shutdown: free the slot and tell the peer.
        if self
            .node
            .tunnels
            .streams
            .lock()
            .unwrap()
            .remove(&self.id)
            .is_some()
            && !self.done
        {
            send_reset(
                &self.ctl,
                self.id,
                &BridgeError::new("STREAM_ABORTED", "Stream aborted"),
            );
        }
    }
}

/// Keeps one listener per configured forward; restarts them when settings change.
pub async fn supervise(node: Arc<Node>) {
    loop {
        let forwards = node.cfg().forwards;
        *node.tunnels.listeners.lock().unwrap() = forwards
            .iter()
            .map(|f| ListenerStatus {
                listen: f.listen.clone(),
                target: f.target.clone(),
                listening: false,
                error: None,
            })
            .collect();
        let stop = node.shutdown.child_token();
        let tasks: Vec<_> = forwards
            .into_iter()
            .enumerate()
            .map(|(index, f)| tokio::spawn(listen(node.clone(), index, f, stop.clone())))
            .collect();
        tokio::select! {
            _ = node.shutdown.cancelled() => {}
            _ = node.tunnels.changed.notified() => {}
        }
        stop.cancel();
        for t in tasks {
            let _ = t.await;
        }
        if node.shutdown.is_cancelled() {
            break;
        }
    }
}

async fn listen(node: Arc<Node>, index: usize, forward: Forward, stop: CancellationToken) {
    let set = |listening: bool, error: Option<String>| {
        if let Some(s) = node.tunnels.listeners.lock().unwrap().get_mut(index) {
            s.listening = listening;
            s.error = error;
        }
    };
    // A port may be briefly busy (old process, previous listener); keep retrying
    // until it binds or the rules change.
    let mut logged = false;
    let listener = loop {
        match TcpListener::bind(forward.listen.as_str()).await {
            Ok(l) => break l,
            Err(e) => {
                set(false, Some(e.to_string()));
                if !logged {
                    node.log(
                        "error",
                        "tunnel",
                        format!("Cannot listen on {}: {e}; retrying", forward.listen),
                        "",
                    );
                    logged = true;
                }
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                }
            }
        }
    };
    set(true, None);
    let local = listener
        .local_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(true);
    node.log(
        if local { "info" } else { "warn" },
        "tunnel",
        if local {
            format!("Forwarding {} → peer {}", forward.listen, forward.target)
        } else {
            format!("Forwarding {} → peer {} (listening beyond loopback; anyone who can reach this port can use the tunnel)", forward.listen, forward.target)
        },
        "",
    );
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((socket, client)) => { tokio::spawn(node.clone().outbound(socket, forward.clone(), client)); }
                Err(e) => {
                    node.log("warn", "tunnel", format!("Accept on {} failed: {e}", forward.listen), "");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
    set(false, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_parse_host_ipv4_and_bracketed_ipv6() {
        assert_eq!(
            split_target("127.0.0.1:443").unwrap(),
            ("127.0.0.1".into(), 443)
        );
        assert_eq!(
            split_target("example.com:22").unwrap(),
            ("example.com".into(), 22)
        );
        assert_eq!(split_target("[::1]:8080").unwrap(), ("::1".into(), 8080));
        for bad in [
            "",
            "host",
            "host:0",
            ":80",
            "::1:80",
            "[::1]80",
            "a b:80",
            "h:65536",
            "user@h:80",
        ] {
            assert!(split_target(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn forwards_require_unique_socket_listen_addresses() {
        let f = |l: &str| Forward {
            listen: l.into(),
            target: "127.0.0.1:443".into(),
        };
        assert!(validate_forwards(&[f("127.0.0.1:10443"), f("127.0.0.1:10444")]).is_ok());
        assert!(validate_forwards(&[f("127.0.0.1:10443"), f("127.0.0.1:10443")]).is_err());
        assert!(validate_forwards(&[f("localhost:10443")]).is_err());
        assert!(validate_forwards(&[f("127.0.0.1:0")]).is_err());
    }
}
