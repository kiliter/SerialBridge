use crate::{
    error::{BridgeError, Result},
    link::Link,
    node::{Node, PeerLink, Reply},
    protocol::{self, FrameCodec, Packet},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, Notify, Semaphore},
    time::Instant,
};
use tokio_serial::SerialPortBuilderExt;
use tokio_util::{codec::FramedRead, sync::CancellationToken};
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
                    // 8N1: ten line bits per byte.
                    session_with_rate(node.clone(), serial, Some(cfg.baud_rate as u64 / 10)).await
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
        let restart = tokio::select! {
            _=node.shutdown.cancelled()=>break,
            result=attempt=>match result{
                // 会话只在手动重连或关停时正常结束。手动重连必须立即用当前配置
                // 重建链路：reconnect 通知已被旧会话消费，等待循环里没有任何
                // 唤醒源，auto_reconnect=false 时否则会话永久离线。
                Ok(())=>!node.shutdown.is_cancelled(),
                Err(e)=>{node.log("warn","serial",format!("{}: {}",e.code,e.message),"");false}
            },
        };
        if restart {
            continue;
        }
        loop {
            // 在线修改自动重连只更新等待策略，不中断当前会话。
            tokio::select! {
                _=node.shutdown.cancelled()=>return,
                _=node.reconnect.notified()=>break,
                _=node.reconnect_policy_changed.notified()=>{},
                _=tokio::time::sleep(Duration::from_secs(3)), if node.cfg().auto_reconnect=>break,
            }
        }
    }
}
/// Runs one link session over an arbitrary byte pipe whose line rate is unknown.
pub async fn session<T>(node: Arc<Node>, io: T) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    session_with_rate(node, io, None).await
}

/// Bytes written per write call; small batches keep retransmits and control
/// frames from queuing behind a large buffered write.
const WRITE_BATCH: usize = 16 * 1024;
/// Interval for aggregated link-error log lines.
const STATS_LOG: Duration = Duration::from_secs(10);

/// Runs one link session. `rate` (bytes/second) scales retransmit timeouts.
pub async fn session_with_rate<T>(node: Arc<Node>, io: T, rate: Option<u64>) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let epoch = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel::<Packet>(32);
    let (ctl, mut ctl_rx) = mpsc::channel::<Packet>(256);
    let link = Arc::new(Mutex::new(Link::new(rate, Instant::now())));
    let wake = Arc::new(Notify::new());
    *node.peer.lock().unwrap() = Some(PeerLink {
        epoch,
        tx: tx.clone(),
        ctl: ctl.clone(),
        ready: false,
        remote: Value::Null,
        link: Some(link.clone()),
    });
    let (read_half, mut write_half) = tokio::io::split(io);
    let mut stream = FramedRead::new(read_half, FrameCodec);
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
    // Poll writing independently of reading. Otherwise two full-duplex peers can
    // both fill the device buffer and wait forever without reading each other.
    // The future remains owned by this session and is dropped on every exit.
    let writer = {
        let link = link.clone();
        let wake = wake.clone();
        let node = node.clone();
        async move {
            // Packets taken from the queues but not yet admitted by the window.
            let mut ctl_slot: Option<Packet> = None;
            let mut bulk_slot: Option<Packet> = None;
            let mut buf = BytesMut::new();
            loop {
                let deadline = {
                    let mut l = link.lock().unwrap();
                    let now = Instant::now();
                    l.check(now)?;
                    loop {
                        // Stream control/acks first so bulk data cannot starve flow control.
                        if ctl_slot.is_none() {
                            ctl_slot = ctl_rx.try_recv().ok();
                        }
                        if ctl_slot.is_some() && l.can_send(true) {
                            l.push(ctl_slot.take().unwrap())?;
                            continue;
                        }
                        if bulk_slot.is_none() {
                            bulk_slot = rx.try_recv().ok();
                        }
                        if ctl_slot.is_none() && bulk_slot.is_some() && l.can_send(false) {
                            l.push(bulk_slot.take().unwrap())?;
                            continue;
                        }
                        break;
                    }
                    while buf.len() < WRITE_BATCH
                        && l.next_out(now, || node.announcement(), &mut buf)?
                    {}
                    l.deadline(now)
                };
                if !buf.is_empty() {
                    tokio::time::timeout(Duration::from_secs(20), async {
                        write_half.write_all(&buf).await?;
                        write_half.flush().await
                    })
                    .await
                    .map_err(|_| BridgeError::new("TIMEOUT", "Serial write timeout"))??;
                    buf.clear();
                    continue;
                }
                tokio::select! {
                    _ = wake.notified() => {},
                    _ = tokio::time::sleep_until(deadline) => {},
                    Some(p) = ctl_rx.recv(), if ctl_slot.is_none() => ctl_slot = Some(p),
                    p = rx.recv(), if bulk_slot.is_none() => match p {
                        Some(p) => bulk_slot = Some(p),
                        None => break,
                    },
                }
            }
            Err::<(), _>(BridgeError::new(
                "PEER_DISCONNECTED",
                "Serial write queue closed",
            ))
        }
    };
    tokio::pin!(writer);
    let mut stats_tick = tokio::time::interval(STATS_LOG);
    let mut logged = crate::link::Stats::default();
    loop {
        tokio::select! {
            _=node.shutdown.cancelled()=>return Ok(()),
            _=node.reconnect.notified()=>return Ok(()),
            result=&mut writer=>return result,
            Some(id)=complete_rx.recv()=>{active.remove(&id);},
            _=stats_tick.tick()=>{
                let now=link.lock().unwrap().stats;
                if now!=logged{
                    node.log("info","serial",format!("Link recovered from line errors: {} damaged frames, {} NAKs sent, {} retransmits (total)",now.damaged,now.naks_sent,now.retransmits),epoch);
                    logged=now;
                }
            },
            received=stream.next()=>{
                let wire=received.ok_or_else(||BridgeError::new("PEER_DISCONNECTED","Serial stream ended"))??;
                let incoming={let mut l=link.lock().unwrap();l.on_wire(wire,Instant::now())?};
                wake.notify_one();
                if let Some(meta)=incoming.hello{
                    if meta["protocol"]!=2||meta["instance"]==json!(node.instance){return Err(BridgeError::invalid("Unsupported protocol or loopback peer"))}
                    let mut p=node.peer.lock().unwrap();
                    if let Some(p)=p.as_mut(){
                        if incoming.adopted{node.log("info","serial","Peer handshake complete",epoch);p.ready=true;}
                        p.remote=meta;
                    }
                }else if incoming.adopted{
                    // Adopted through a data frame; HELLO metadata arrived earlier.
                    let mut p=node.peer.lock().unwrap();
                    if let Some(p)=p.as_mut(){node.log("info","serial","Peer handshake complete",epoch);p.ready=true;}
                }
                for packet in incoming.deliver{
                match packet.kind {
                    protocol::RESPONSE=>{
                        let entry={let mut pending=node.pending.lock().unwrap();if pending.get(&packet.id).is_some_and(|p|p.epoch==epoch){pending.remove(&packet.id)}else{None}};
                        if let Some(p)=entry{let result=if let Some(err)=packet.meta.get("error"){Err(serde_json::from_value(err.clone()).unwrap_or_else(|_|BridgeError::invalid("Invalid peer error")))}else{Ok(Reply{value:packet.meta["result"].clone(),data:packet.data})};let _=p.tx.send(result);}
                    },
                    protocol::CANCEL=>{if let Some(c)=active.remove(&packet.id){c.cancel();}},
                    protocol::REQUEST=>{
                        if active.contains_key(&packet.id){return Err(BridgeError::invalid("Duplicate active request ID"))}
                        let permit=match slots.clone().try_acquire_owned(){Ok(p)=>p,Err(_)=>{let error=BridgeError::new("BUSY","Peer execution limit reached");crate::tunnel::send_control(&ctl,Packet::new(protocol::RESPONSE,packet.id,json!({"error":error}),Bytes::new()));continue}};
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
                    protocol::STREAM_OPEN..=protocol::STREAM_RESET=>{
                        node.tunnel_packet(epoch,&ctl,packet)?;
                    },
                    _=>return Err(BridgeError::invalid("Invalid packet kind")),
                }
                }
            }
        }
    }
}
