use crate::{
    config::Config,
    error::{BridgeError, Result},
    protocol::{self, Packet},
    transfer::Transfers,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub value: Value,
    pub data: Bytes,
}
impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            value,
            data: Bytes::new(),
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    #[serde(default = "peer_target")]
    pub target: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default = "rpc_timeout")]
    pub timeout_ms: u64,
}
fn peer_target() -> String {
    "peer".into()
}
fn rpc_timeout() -> u64 {
    60000
}
#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    pub time: u64,
    pub level: String,
    pub component: String,
    pub message: String,
    pub id: String,
}
#[derive(Clone)]
pub struct PeerLink {
    pub epoch: Uuid,
    pub tx: mpsc::Sender<Packet>,
    /// High-priority queue (stream control/acks), drained before `tx`.
    pub ctl: mpsc::Sender<Packet>,
    pub ready: bool,
    pub remote: Value,
}
pub struct PeerPending {
    pub epoch: Uuid,
    pub tx: oneshot::Sender<Result<Reply>>,
}
#[derive(Clone)]
pub struct BrowserLink {
    pub epoch: Uuid,
    pub tx: mpsc::Sender<Value>,
    pub stop: CancellationToken,
    pub origin: String,
}
pub enum BrowserEvent {
    Meta(Value),
    Chunk(Value, Bytes),
}
pub struct BrowserPending {
    pub epoch: Uuid,
    pub tx: mpsc::Sender<BrowserEvent>,
}
pub struct PairCode {
    pub code: String,
    pub expires: std::time::Instant,
    pub attempts: u8,
}
pub struct Node {
    pub config: RwLock<Config>,
    pub config_path: PathBuf,
    pub instance: Uuid,
    pub listen_port: u16,
    pub peer: Mutex<Option<PeerLink>>,
    pub pending: Mutex<HashMap<Uuid, PeerPending>>,
    pub browser: Mutex<Option<BrowserLink>>,
    pub browser_pending: Mutex<HashMap<Uuid, BrowserPending>>,
    pub pairing: Mutex<Option<PairCode>>,
    pub logs: Mutex<VecDeque<LogEvent>>,
    pub transfers: Transfers,
    pub tunnels: crate::tunnel::Tunnels,
    pub shutdown: CancellationToken,
    pub reconnect: tokio::sync::Notify,
    pub reconnect_policy_changed: tokio::sync::Notify,
    pub client: reqwest::Client,
}
impl Node {
    pub fn new(config: Config, path: PathBuf) -> Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(BridgeError::io)?;
        Ok(Arc::new(Self {
            listen_port: config.http_port,
            config: RwLock::new(config),
            config_path: path,
            instance: Uuid::new_v4(),
            peer: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            browser: Mutex::new(None),
            browser_pending: Mutex::new(HashMap::new()),
            pairing: Mutex::new(None),
            logs: Mutex::new(VecDeque::new()),
            transfers: Transfers::default(),
            tunnels: Default::default(),
            shutdown: CancellationToken::new(),
            reconnect: tokio::sync::Notify::new(),
            reconnect_policy_changed: tokio::sync::Notify::new(),
            client,
        }))
    }
    pub fn cfg(&self) -> Config {
        self.config.read().unwrap().clone()
    }
    pub fn log(&self, level: &str, component: &str, message: impl Into<String>, id: impl ToString) {
        let message = message.into();
        let id = id.to_string();
        tracing::info!(level,component,request_id=%id,"{}",message);
        let mut logs = self.logs.lock().unwrap();
        logs.push_back(LogEvent {
            time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            level: level.into(),
            component: component.into(),
            message,
            id,
        });
        while logs.len() > 2000 {
            logs.pop_front();
        }
    }
    pub fn announcement(&self) -> Value {
        let cfg = self.cfg();
        json!({"protocol":1,"instance":self.instance,"name":cfg.node_name,"os":std::env::consts::OS,"browser":self.browser.lock().unwrap().is_some(),"tunnel":1})
    }
    pub fn snapshot(&self) -> Value {
        let cfg = self.cfg();
        let link = self.peer.lock().unwrap().clone();
        let peer = match link {
            Some(p) if p.ready => {
                json!({"connected":true,"name":p.remote["name"],"os":p.remote["os"],"browser":p.remote["browser"],"session":p.epoch})
            }
            _ => json!({"connected":false,"browser":false}),
        };
        let browser = self.browser.lock().unwrap().clone();
        json!({"version":env!("CARGO_PKG_VERSION"),"node":{"name":cfg.node_name,"os":std::env::consts::OS,"instance":self.instance},"http":{"listening":true,"port":self.listen_port},"serial":{"port":cfg.serial_port,"baud_rate":cfg.baud_rate},"peer":peer,"browser":{"connected":browser.is_some(),"origin":browser.map(|b|b.origin)},"pending_requests":self.pending.lock().unwrap().len(),"tunnel":{"forwards":cfg.forwards.len(),"active_streams":self.tunnels.streams.lock().unwrap().len()}})
    }
    pub fn update_settings(&self, patch: Value) -> Result<Value> {
        let obj = patch
            .as_object()
            .ok_or_else(|| BridgeError::invalid("Expected settings object"))?;
        let allowed = [
            "node_name",
            "serial_port",
            "baud_rate",
            "allowed_origins",
            "allowed_methods",
            "auto_reconnect",
            "http_port",
            "chunk_bytes",
            "max_file_bytes",
            "max_spool_bytes",
            "forwards",
        ];
        if obj.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(BridgeError::invalid(
                "Setting cannot be changed through the API",
            ));
        }
        let mut lock = self.config.write().unwrap();
        let mut value = serde_json::to_value(&*lock)?;
        for (k, v) in obj {
            value[k] = v.clone();
        }
        let next: Config = serde_json::from_value(value)?;
        next.validate()?;
        let restart = next.http_port != self.listen_port;
        let reconnect = next.serial_port != lock.serial_port || next.baud_rate != lock.baud_rate;
        let policy_changed = next.auto_reconnect != lock.auto_reconnect;
        let forwards_changed = next.forwards != lock.forwards;
        next.save(&self.config_path)?;
        // A listening socket cannot change address in place; keep runtime port truthful until restart.
        *lock = next.clone();
        drop(lock);
        if reconnect {
            self.reconnect.notify_one();
        }
        if policy_changed {
            self.reconnect_policy_changed.notify_one();
        }
        if forwards_changed {
            self.tunnels.changed.notify_one();
        }
        self.log("info", "settings", "Settings saved", "");
        Ok(json!({"settings":next.public(),"restart_required":restart}))
    }
    pub async fn call(
        self: &Arc<Self>,
        target: &str,
        method: &str,
        params: Value,
        data: Bytes,
        timeout_ms: u64,
    ) -> Result<Reply> {
        match target {
            "local" => tokio::time::timeout(
                Duration::from_millis(timeout_ms.clamp(1000, 120000)),
                self.execute_local(method.to_string(), params, data),
            )
            .await
            .map_err(|_| BridgeError::new("TIMEOUT", "Local request timed out"))?,
            "peer" => self.peer_call(method, params, data, timeout_ms).await,
            _ => Err(BridgeError::invalid("target must be local or peer")),
        }
    }
    pub async fn peer_call(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        data: Bytes,
        timeout_ms: u64,
    ) -> Result<Reply> {
        let link = self
            .peer
            .lock()
            .unwrap()
            .clone()
            .filter(|p| p.ready)
            .ok_or_else(|| BridgeError::new("PEER_DISCONNECTED", "Serial peer is not ready"))?;
        let meta = json!({"method":method,"params":params});
        if serde_json::to_vec(&meta)?.len() + data.len() > protocol::MAX_PAYLOAD {
            return Err(BridgeError::new(
                "REQUEST_TOO_LARGE",
                "Request exceeds serial frame limit",
            ));
        }
        let id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= 64 {
                return Err(BridgeError::new("BUSY", "Too many pending requests"));
            }
            pending.insert(
                id,
                PeerPending {
                    epoch: link.epoch,
                    tx,
                },
            );
        }
        let _guard = PeerGuard {
            node: self.clone(),
            id,
            tx: link.tx.clone(),
        };
        let packet = Packet::new(protocol::REQUEST, id, meta, data);
        link.tx
            .try_send(packet)
            .map_err(|_| BridgeError::new("BUSY", "Serial write queue unavailable"))?;
        self.log("info", "rpc", format!("Sent {method}"), id);
        tokio::time::timeout(Duration::from_millis(timeout_ms.clamp(1000, 120000)), rx)
            .await
            .map_err(|_| {
                BridgeError::new(
                    "TIMEOUT",
                    "Peer request timed out; execution outcome may be unknown",
                )
            })?
            .map_err(|_| BridgeError::new("PEER_DISCONNECTED", "Peer disconnected"))?
    }
    pub fn peer_lost(self: &Arc<Self>, epoch: Uuid) {
        {
            let mut p = self.peer.lock().unwrap();
            if p.as_ref().is_some_and(|p| p.epoch == epoch) {
                *p = None;
            }
        }
        self.pending.lock().unwrap().retain(|_, p| p.epoch != epoch);
        self.tunnels_lost(epoch);
        let jobs = self.transfers.cancel_peer(epoch);
        let node = self.clone();
        tokio::spawn(async move {
            for id in jobs {
                let _ = node.cancel_transfer(id).await;
            }
        });
        self.log(
            "warn",
            "serial",
            "Peer disconnected; pending requests failed, no automatic replay",
            epoch,
        );
    }
    pub(crate) async fn execute_peer(
        self: &Arc<Self>,
        epoch: Uuid,
        method: String,
        params: Value,
        data: Bytes,
    ) -> Result<Reply> {
        match method.as_str() {
            "download.start" => self.start_source_download_owned(params, Some(epoch)),
            "file.create" => self.create_upload_owned(params, Some(epoch)).await,
            "file.open" => self.open_shared_owned(params, Some(epoch)).await,
            _ => self.execute_local(method, params, data).await,
        }
    }
    pub fn execute_local<'a>(
        self: &'a Arc<Self>,
        method: String,
        params: Value,
        data: Bytes,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Reply>> + Send + 'a>> {
        Box::pin(async move {
            match method.as_str() {
                "system.ping" => Ok(Reply::json(self.announcement())),
                "http.fetch" => crate::http_executor::fetch(self, params).await,
                "browser.fetch" => self.browser_fetch(params).await,
                "download.start" => self.start_source_download(params),
                "download.status" => self.transfer_status_reply(params),
                "download.cancel" => self.cancel_transfer_reply(params).await,
                "file.open" => self.open_shared(params).await,
                "file.read" => self.read_block(params).await,
                "file.create" => self.create_upload(params).await,
                "file.write" => self.write_block(params, data).await,
                "file.finish" => self.finish_upload(params).await,
                "file.remove" => self.remove_artifact(params).await,
                _ => Err(BridgeError::new("METHOD_NOT_FOUND", "Unknown RPC method")),
            }
        })
    }
    pub async fn begin_browser(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<BrowserTask> {
        let link = self.browser.lock().unwrap().clone().ok_or_else(|| {
            BridgeError::new("BROWSER_DISCONNECTED", "Local browser extension is offline")
        })?;
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(4);
        {
            let mut pending = self.browser_pending.lock().unwrap();
            if pending.len() >= 8 {
                return Err(BridgeError::new(
                    "BUSY",
                    "Browser concurrency limit reached",
                ));
            }
            pending.insert(
                id,
                BrowserPending {
                    epoch: link.epoch,
                    tx,
                },
            );
        }
        let task = BrowserTask {
            node: self.clone(),
            id,
            tx: link.tx.clone(),
            rx,
        };
        link.tx.try_send(json!({"type":"task.execute","id":id,"method":method,"params":params,"policy":{"origins":self.cfg().allowed_origins,"methods":self.cfg().allowed_methods,"maxInlineBytes":16384,"maxFileBytes":self.cfg().max_file_bytes}})).map_err(|_|BridgeError::new("BROWSER_DISCONNECTED","Browser queue unavailable"))?;
        Ok(task)
    }
    pub async fn browser_fetch(self: &Arc<Self>, params: Value) -> Result<Reply> {
        crate::http_executor::validate(&self.cfg(), &params)?;
        let mut task = self.begin_browser("browser.fetch", params).await?;
        match task.next().await? {
            BrowserEvent::Meta(v) => {
                if v["type"] == "task.error" {
                    return Err(BridgeError::new(
                        "BROWSER_ERROR",
                        v["message"].as_str().unwrap_or("Browser task failed"),
                    ));
                }
                if v["type"] != "task.result" {
                    return Err(BridgeError::invalid("Unexpected browser response"));
                }
                Ok(Reply::json(v["result"].clone()))
            }
            _ => Err(BridgeError::invalid("Unexpected binary response")),
        }
    }
}
struct PeerGuard {
    node: Arc<Node>,
    id: Uuid,
    tx: mpsc::Sender<Packet>,
}
impl Drop for PeerGuard {
    fn drop(&mut self) {
        if self.node.pending.lock().unwrap().remove(&self.id).is_some() {
            let _ = self.tx.try_send(Packet::new(
                protocol::CANCEL,
                self.id,
                json!({}),
                Bytes::new(),
            ));
        }
    }
}
pub struct BrowserTask {
    pub node: Arc<Node>,
    pub id: Uuid,
    pub tx: mpsc::Sender<Value>,
    pub rx: mpsc::Receiver<BrowserEvent>,
}
impl BrowserTask {
    pub async fn next(&mut self) -> Result<BrowserEvent> {
        tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
            .await
            .map_err(|_| BridgeError::new("TIMEOUT", "Browser did not produce data in 60 seconds"))?
            .ok_or_else(|| {
                BridgeError::new("BROWSER_DISCONNECTED", "Browser extension disconnected")
            })
    }
    pub fn ack(&self, offset: u64) -> Result<()> {
        self.tx
            .try_send(json!({"type":"chunk.ack","id":self.id,"offset":offset}))
            .map_err(|_| BridgeError::new("BROWSER_DISCONNECTED", "Browser acknowledgement failed"))
    }
}
impl Drop for BrowserTask {
    fn drop(&mut self) {
        self.node.browser_pending.lock().unwrap().remove(&self.id);
        let _ = self.tx.try_send(json!({"type":"task.cancel","id":self.id}));
    }
}
