mod support;
use serde_json::json;
use serialbridge_core::tunnel;
use std::time::Duration;
use support::Pair;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Echo server on B's side; returns its address.
async fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    addr
}

/// Configures one forward on A and waits until it listens.
async fn forward(pair: &Pair, target: &str) -> String {
    tokio::spawn(tunnel::supervise(pair.a.node.clone()));
    let listen = format!("127.0.0.1:{}", free_port());
    pair.a
        .node
        .update_settings(json!({"forwards":[{"listen":listen,"target":target}]}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while pair.a.node.tunnel_snapshot()["forwards"][0]["listening"] != true {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("forward listening");
    listen
}

async fn wait_streams(node: &serialbridge_core::node::Node, n: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while node.tunnels.streams.lock().unwrap().len() != n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected {n} streams, have {}",
            node.tunnels.streams.lock().unwrap().len()
        )
    });
}

#[tokio::test]
async fn bytes_round_trip_beyond_window_through_small_link_buffer() {
    let pair = Pair::new(4096).await;
    let listen = forward(&pair, &echo_server().await).await;
    let data: Vec<u8> = (0..(1024 * 1024 + 17))
        .map(|i| (i * 31 % 251) as u8)
        .collect();
    let mut s = TcpStream::connect(&listen).await.unwrap();
    let (mut r, mut w) = s.split();
    let send = async {
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let recv = r.read_to_end(&mut back);
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(send, recv).1.unwrap()
    })
    .await
    .expect("relay finishes");
    assert_eq!(back.len(), data.len());
    assert!(back == data, "payload must be byte-identical");
    wait_streams(&pair.a.node, 0).await;
    wait_streams(&pair.b.node, 0).await;
}

#[tokio::test]
async fn concurrent_streams_are_independent() {
    let pair = Pair::new(4096).await;
    let listen = forward(&pair, &echo_server().await).await;
    let mut tasks = Vec::new();
    for i in 0..8u8 {
        let listen = listen.clone();
        tasks.push(tokio::spawn(async move {
            let mut s = TcpStream::connect(&listen).await.unwrap();
            let msg = vec![i; 50_000];
            s.write_all(&msg).await.unwrap();
            s.shutdown().await.unwrap();
            let mut back = Vec::new();
            s.read_to_end(&mut back).await.unwrap();
            assert_eq!(back, msg);
        }));
    }
    for t in tasks {
        tokio::time::timeout(Duration::from_secs(20), t)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(pair.a.node.tunnel_snapshot()["total_streams"], 8);
}

#[tokio::test]
async fn half_close_keeps_the_other_direction_open() {
    let pair = Pair::new(4096).await;
    // Server reads everything, then answers after the client's FIN.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).await.unwrap();
        s.write_all(format!("got {}", got.len()).as_bytes())
            .await
            .unwrap();
    });
    let listen = forward(&pair, &target).await;
    let mut s = TcpStream::connect(&listen).await.unwrap();
    s.write_all(b"hello").await.unwrap();
    s.shutdown().await.unwrap();
    let mut back = String::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back, "got 5");
}

#[tokio::test]
async fn unreachable_target_closes_client_and_frees_slots() {
    let pair = Pair::new(4096).await;
    let listen = forward(&pair, &format!("127.0.0.1:{}", free_port())).await;
    let mut s = TcpStream::connect(&listen).await.unwrap();
    let mut buf = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf))
        .await
        .unwrap();
    assert!(n.map(|n| n == 0).unwrap_or(true));
    wait_streams(&pair.a.node, 0).await;
    wait_streams(&pair.b.node, 0).await;
    assert!(pair
        .a
        .node
        .logs
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.component == "tunnel" && l.message.contains("failed")));
}

#[tokio::test]
async fn link_loss_resets_forwarded_connections() {
    let mut pair = Pair::new(4096).await;
    let listen = forward(&pair, &echo_server().await).await;
    let mut s = TcpStream::connect(&listen).await.unwrap();
    s.write_all(b"ping").await.unwrap();
    let mut four = [0u8; 4];
    s.read_exact(&mut four).await.unwrap();
    wait_streams(&pair.b.node, 1).await;
    pair.disconnect().await;
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest))
        .await
        .expect("client closed");
    wait_streams(&pair.a.node, 0).await;
    wait_streams(&pair.b.node, 0).await;
}

#[tokio::test]
async fn link_loss_releases_a_stream_after_receiving_fin() {
    let mut pair = Pair::new(4096).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = forward(&pair, &listener.local_addr().unwrap().to_string()).await;
    let mut client = TcpStream::connect(&listen).await.unwrap();
    let (mut target, _) = listener.accept().await.unwrap();
    client.shutdown().await.unwrap();
    let mut data = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), target.read_to_end(&mut data))
        .await
        .unwrap()
        .unwrap();
    let retained = {
        let streams = pair.b.node.tunnels.streams.lock().unwrap();
        std::sync::Arc::downgrade(&streams.values().next().unwrap().info)
    };
    pair.disconnect().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while retained.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("断链后半关闭的转发任务仍持有目标 socket");
}

#[tokio::test]
async fn closing_the_client_aborts_the_peer_side() {
    let pair = Pair::new(4096).await;
    // Target never closes on its own.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap().to_string();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf).await;
        let _ = closed_tx.send(());
    });
    let listen = forward(&pair, &target).await;
    let s = TcpStream::connect(&listen).await.unwrap();
    wait_streams(&pair.b.node, 1).await;
    drop(s);
    tokio::time::timeout(Duration::from_secs(5), closed_rx)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn invalid_forwards_are_rejected_by_settings() {
    let pair = Pair::new(4096).await;
    for forwards in [
        json!([{"listen":"127.0.0.1:10443","target":"no-port"}]),
        json!([{"listen":"0:1","target":"127.0.0.1:443"}]),
        json!([{"listen":"127.0.0.1:10443","target":"a:1"},{"listen":"127.0.0.1:10443","target":"b:1"}]),
    ] {
        let err = pair
            .a
            .node
            .update_settings(json!({ "forwards": forwards }))
            .unwrap_err();
        assert_eq!(err.code, "INVALID_REQUEST");
    }
    assert!(pair.a.node.cfg().forwards.is_empty());
}

#[tokio::test]
async fn a_stalled_stream_does_not_block_other_streams() {
    let mut pair = Pair::new(4096).await;
    let echo = echo_server().await;
    // Second target accepts but never reads, so its window fills up.
    let stall = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_target = stall.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (s, _) = stall.accept().await.unwrap();
            held.push(s);
        }
    });
    tokio::spawn(tunnel::supervise(pair.a.node.clone()));
    let (l1, l2) = (
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    );
    pair.a
        .node
        .update_settings(
            json!({"forwards":[{"listen":l1,"target":echo},{"listen":l2,"target":stall_target}]}),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while pair.a.node.tunnel_snapshot()["forwards"][1]["listening"] != true {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut stalled = TcpStream::connect(&l2).await.unwrap();
    let flood = tokio::spawn(async move {
        let junk = vec![7u8; 64 * 1024];
        // Blocks once the target's socket buffers and our window are full.
        loop {
            if stalled.write_all(&junk).await.is_err() {
                break;
            }
        }
    });
    // Let the stalled stream exhaust its credit, then confirm it really stalled.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let sent = |node: &serialbridge_core::node::Node| {
        node.tunnel_snapshot()["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["bytes_sent"].as_u64().unwrap())
            .max()
            .unwrap_or(0)
    };
    let before = sent(&pair.a.node);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!flood.is_finished());
    assert_eq!(
        sent(&pair.a.node),
        before,
        "stalled stream must stop at its window"
    );
    assert!(before as usize >= tunnel::WINDOW);
    let mut s = TcpStream::connect(&l1).await.unwrap();
    let msg = vec![3u8; 200_000];
    s.write_all(&msg).await.unwrap();
    s.shutdown().await.unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut back))
        .await
        .expect("healthy stream progresses while another is stalled")
        .unwrap();
    assert_eq!(back, msg);
    let retained: Vec<_> = pair
        .b
        .node
        .tunnels
        .streams
        .lock()
        .unwrap()
        .values()
        .map(|s| std::sync::Arc::downgrade(&s.info))
        .collect();
    pair.disconnect().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while retained.iter().any(|s| s.upgrade().is_some()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("断链后阻塞写入的转发任务未退出");
    flood.abort();
}

#[tokio::test]
async fn listener_retries_until_its_port_is_free() {
    let pair = Pair::new(4096).await;
    let echo = echo_server().await;
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = blocker.local_addr().unwrap().to_string();
    tokio::spawn(tunnel::supervise(pair.a.node.clone()));
    pair.a
        .node
        .update_settings(json!({"forwards":[{"listen":listen,"target":echo}]}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while pair.a.node.tunnel_snapshot()["forwards"][0]["error"].is_null() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bind error reported");
    drop(blocker);
    tokio::time::timeout(Duration::from_secs(6), async {
        while pair.a.node.tunnel_snapshot()["forwards"][0]["listening"] != true {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("listener recovers without a rules change");
    let mut s = TcpStream::connect(&listen).await.unwrap();
    s.write_all(b"ok").await.unwrap();
    let mut two = [0u8; 2];
    s.read_exact(&mut two).await.unwrap();
    assert_eq!(&two, b"ok");
}
