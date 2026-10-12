use crate::{
    error::{BridgeError, Result},
    node::{Node, PeerLink, Reply},
    protocol::{self, FrameCodec, Packet},
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, Semaphore},
};
use tokio_serial::SerialPortBuilderExt;
use tokio_util::{codec::Framed, sync::CancellationToken};
use uuid::Uuid;
/// Only for development tests. Production uses Serial; no IP interface or USB networking is created.
#[derive(Clone, Debug)]
pub enum TransportMode {
    Serial,
    TcpConnect(std::net::SocketAddr),
    TcpListen(std::net::SocketAddr),
}
pub async fn run(node: Arc<Node>, mode: TransportMode) {
    loop {
        let cfg = node.cfg();
        let attempt = async {
            match &mode {
                TransportMode::Serial => {
                    if cfg.serial_port.is_empty() {
                        return Err(BridgeError::new(
                            "NO_PORT",
                            "Select a serial port in Settings",
                        ));
                    }
                    let serial = tokio_serial::new(&cfg.serial_port, cfg.baud_rate)
                        .data_bits(tokio_serial::DataBits::Eight)
                        .parity(tokio_serial::Parity::None)
                        .stop_bits(tokio_serial::StopBits::One)
                        .flow_control(tokio_serial::FlowControl::None)
                        .open_native_async()
                        .map_err(BridgeError::io)?;
                    session(node.clone(), serial).await
                }
                TransportMode::TcpConnect(addr) => {
                    if !addr.ip().is_loopback() {
                        return Err(BridgeError::invalid(
                            "Development transport must use loopback",
                        ));
                    }
                    let stream = tokio::net::TcpStream::connect(addr).await?;
                    stream.set_nodelay(true)?;
                    session(node.clone(), stream).await
                }
                TransportMode::TcpListen(addr) => {
                    if !addr.ip().is_loopback() {
                        return Err(BridgeError::invalid(
                            "Development transport must use loopback",
                        ));
                    }
                    let listener = tokio::net::TcpListener::bind(addr).await?;
                    let (stream, _) = listener.accept().await?;
                    stream.set_nodelay(true)?;
                    session(node.clone(), stream).await
                }
            }
        };
        tokio::select! {
            _=node.shutdown.cancelled()=>break,
            result=attempt=>if let Err(e)=result{node.log("warn","serial",format!("{}: {}",e.code,e.message),"");},
        }
        if !cfg.auto_reconnect {
            tokio::select! {_=node.shutdown.cancelled()=>break,_=node.reconnect.notified()=>{}}
        } else {
            tokio::select! {_=node.shutdown.cancelled()=>break,_=node.reconnect.notified()=>{},_=tokio::time::sleep(Duration::from_secs(3))=>{}}
        }
    }
}
pub async fn session<T>(node: Arc<Node>, io: T) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let epoch = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel::<Packet>(32);
    *node.peer.lock().unwrap() = Some(PeerLink {
        epoch,
        tx: tx.clone(),
        ready: false,
        remote: Value::Null,
    });
    let (sink, mut stream) = Framed::new(io, FrameCodec).split();
    let stop = CancellationToken::new();
    // This guard also runs if the outer future is cancelled during shutdown.
    struct Guard {
        node: Arc<Node>,
        epoch: Uuid,
        stop: CancellationToken,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            self.stop.cancel();
            self.node.peer_lost(self.epoch);
        }
    }
    let _guard = Guard {
        node: node.clone(),
        epoch,
        stop: stop.clone(),
    };
    let mut active: HashMap<Uuid, CancellationToken> = HashMap::new();
    let (complete_tx, mut complete_rx) = mpsc::channel::<Uuid>(32);
    let slots = Arc::new(Semaphore::new(16));
    tx.try_send(Packet::new(
        protocol::HELLO,
        epoch,
        node.announcement(),
        Bytes::new(),
    ))
    .map_err(|_| BridgeError::new("PEER_DISCONNECTED", "Serial write queue closed"))?;
    // Poll writing independently of reading. Otherwise two full-duplex peers can
    // both fill the device buffer and wait forever without reading each other.
    // The future remains owned by this session and is dropped on every exit.
    let writer = async move {
        let mut sink = sink;
        while let Some(packet) = rx.recv().await {
            tokio::time::timeout(Duration::from_secs(20), sink.send(packet))
                .await
                .map_err(|_| BridgeError::new("TIMEOUT", "Serial write timeout"))??;
        }
        Err::<(), _>(BridgeError::new(
            "PEER_DISCONNECTED",
            "Serial write queue closed",
        ))
    };
    tokio::pin!(writer);
    let mut remote_epoch = None;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _=node.shutdown.cancelled()=>return Ok(()),
            _=node.reconnect.notified()=>return Ok(()),
            result=&mut writer=>return result,
            Some(id)=complete_rx.recv()=>{active.remove(&id);},
            _=heartbeat.tick()=>{
                if last.elapsed()>Duration::from_secs(25){return Err(BridgeError::new("TIMEOUT","Peer heartbeat timed out"))}
                // Traffic already keeps the link alive; do not block the reader
                // behind a full write queue just to add a heartbeat.
                let _=tx.try_send(Packet::new(protocol::HEARTBEAT,epoch,node.announcement(),Bytes::new()));
            },
            received=stream.next()=>{
                let packet=received.ok_or_else(||BridgeError::new("PEER_DISCONNECTED","Serial stream ended"))??;last=tokio::time::Instant::now();
                match packet.kind {
                    protocol::HELLO|protocol::HEARTBEAT=>{
                        if packet.meta["protocol"]!=1||packet.meta["instance"]==json!(node.instance){return Err(BridgeError::invalid("Unsupported protocol or loopback peer"))}
                        if remote_epoch.is_some_and(|id|id!=packet.id){return Err(BridgeError::new("PEER_DISCONNECTED","Remote session changed; pending calls are not replayed"))}remote_epoch=Some(packet.id);
                        let mut p=node.peer.lock().unwrap();if let Some(p)=p.as_mut(){if !p.ready{node.log("info","serial","Peer handshake complete",epoch)}p.ready=true;p.remote=packet.meta;}
                    },
                    protocol::RESPONSE=>{
                        let entry={let mut pending=node.pending.lock().unwrap();if pending.get(&packet.id).is_some_and(|p|p.epoch==epoch){pending.remove(&packet.id)}else{None}};
                        if let Some(p)=entry{let result=if let Some(err)=packet.meta.get("error"){Err(serde_json::from_value(err.clone()).unwrap_or_else(|_|BridgeError::invalid("Invalid peer error")))}else{Ok(Reply{value:packet.meta["result"].clone(),data:packet.data})};let _=p.tx.send(result);}
                    },
                    protocol::CANCEL=>{if let Some(c)=active.remove(&packet.id){c.cancel();}},
                    protocol::REQUEST=>{
                        let ready=node.peer.lock().unwrap().as_ref().is_some_and(|p|p.ready);
                        if !ready{return Err(BridgeError::invalid("RPC received before handshake"))}
                        if active.contains_key(&packet.id){return Err(BridgeError::invalid("Duplicate active request ID"))}
                        let permit=match slots.clone().try_acquire_owned(){Ok(p)=>p,Err(_)=>{let error=BridgeError::new("BUSY","Peer execution limit reached");let _=tx.try_send(Packet::new(protocol::RESPONSE,packet.id,json!({"error":error}),Bytes::new()));continue}};
                        let method=packet.meta["method"].as_str().ok_or_else(||BridgeError::invalid("Missing RPC method"))?.to_string();
                        let cancel=stop.child_token();active.insert(packet.id,cancel.clone());
                        let n=node.clone();let out=tx.clone();let done=complete_tx.clone();
                        tokio::spawn(async move{
                            let _permit=permit;
                            let result=tokio::select!{_=cancel.cancelled()=>Err(BridgeError::new("CANCELLED","Request cancelled; side effects may have occurred")),r=n.execute_peer(epoch,method,packet.meta["params"].clone(),packet.data)=>r};
                            let reply=match result{Ok(r)=>Packet::new(protocol::RESPONSE,packet.id,json!({"result":r.value}),r.data),Err(e)=>Packet::new(protocol::RESPONSE,packet.id,json!({"error":e}),Bytes::new())};
                            let reply=if serde_json::to_vec(&reply.meta).map(|v|v.len()+reply.data.len()>protocol::MAX_PAYLOAD).unwrap_or(true){Packet::new(protocol::RESPONSE,packet.id,json!({"error":BridgeError::new("RESPONSE_TOO_LARGE","Response exceeds serial frame limit")}),Bytes::new())}else{reply};
                            if !cancel.is_cancelled(){let _=out.send(reply).await;}let _=done.send(packet.id).await;
                        });
                    },
                    _=>return Err(BridgeError::invalid("Invalid packet kind")),
                }
            }
        }
    }
}
