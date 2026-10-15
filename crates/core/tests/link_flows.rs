//! 链路层重传：串口线上的随机位翻转不能拆掉会话，数据必须逐字节一致。
mod support;
use bytes::Bytes;
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

fn session(pair: &Pair) -> (serde_json::Value, serde_json::Value) {
    (
        pair.a.node.snapshot()["peer"]["session"].clone(),
        pair.b.node.snapshot()["peer"]["session"].clone(),
    )
}

fn disconnected(pair: &Pair) -> bool {
    [&pair.a, &pair.b].iter().any(|n| {
        n.node
            .logs
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.message.starts_with("Peer disconnected"))
    })
}

#[tokio::test]
async fn corrupted_frames_are_retransmitted_without_dropping_the_session() {
    let pair = Pair::with_noise(4096, 20_000).await;
    let before = session(&pair);
    let listen = forward(&pair, &echo_server().await).await;
    let data: Vec<u8> = (0..(1024 * 1024 + 7))
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
    tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(send, recv).1.unwrap()
    })
    .await
    .expect("relay finishes despite line noise");
    assert_eq!(back.len(), data.len());
    assert!(back == data, "payload must be byte-identical");
    assert!(
        !disconnected(&pair),
        "line noise must not reset the session"
    );
    assert_eq!(session(&pair), before);
    assert!(pair.a.node.snapshot()["peer"]["link"]["retransmits"].as_u64() > Some(0));
}

#[tokio::test]
async fn heavy_noise_with_concurrent_streams() {
    let pair = Pair::with_noise(4096, 40_000).await;
    let listen = forward(&pair, &echo_server().await).await;
    let mut tasks = Vec::new();
    for i in 0..8u8 {
        let listen = listen.clone();
        tasks.push(tokio::spawn(async move {
            let mut s = TcpStream::connect(&listen).await.unwrap();
            let msg: Vec<u8> = (0..100_000)
                .map(|k| (k as u8).wrapping_mul(i + 1))
                .collect();
            s.write_all(&msg).await.unwrap();
            s.shutdown().await.unwrap();
            let mut back = Vec::new();
            s.read_to_end(&mut back).await.unwrap();
            assert!(back == msg);
        }));
    }
    for t in tasks {
        tokio::time::timeout(Duration::from_secs(60), t)
            .await
            .expect("stream finishes under heavy noise")
            .unwrap();
    }
    assert!(!disconnected(&pair));
}

#[tokio::test]
async fn rpc_survives_line_noise() {
    let pair = Pair::with_noise(4096, 3_000).await;
    for n in 0..200 {
        let reply = pair
            .a
            .node
            .peer_call(
                "system.ping",
                json!({}),
                Bytes::from(vec![n as u8; 1024]),
                20_000,
            )
            .await
            .unwrap_or_else(|e| panic!("ping {n}: {}", e.message));
        assert_eq!(reply.value["protocol"], 2);
    }
    assert!(!disconnected(&pair));
}
