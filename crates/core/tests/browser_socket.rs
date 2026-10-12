mod support;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use serialbridge_core::{api, node::Node};
use std::{sync::Arc, time::Duration};
use support::TestNode;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const ORIGIN: &str = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Server {
    fixture: TestNode,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new() -> Self {
        let fixture = TestNode::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = api::router(fixture.node.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            fixture,
            port,
            task,
        }
    }
    async fn socket(&self, origin: &str, first: Value) -> (Socket, Value) {
        let mut request = format!("ws://127.0.0.1:{}/v1/extension/ws", self.port)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        socket
            .send(Message::Text(first.to_string().into()))
            .await
            .unwrap();
        let ready = message(&mut socket).await;
        (socket, ready)
    }
    async fn paired(&self) -> (Socket, Value) {
        let code = self.fixture.node.new_pair_code()["code"].clone();
        self.socket(ORIGIN, json!({"type":"pair","code":code}))
            .await
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.fixture.node.shutdown.cancel();
        self.task.abort();
    }
}
async fn message(socket: &mut Socket) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "ping" {
                        socket
                            .send(Message::Text(json!({"type":"pong"}).to_string().into()))
                            .await
                            .unwrap();
                        continue;
                    }
                    return value;
                }
                Message::Ping(data) => socket.send(Message::Pong(data)).await.unwrap(),
                other => panic!("unexpected socket message {other:?}"),
            }
        }
    })
    .await
    .expect("browser control message")
}
async fn completed(node: &Arc<Node>, id: uuid::Uuid) -> serialbridge_core::transfer::JobView {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = node.transfers.get(id).unwrap();
            if ["completed", "failed", "cancelled"].contains(&view.status.as_str()) {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn actual_websocket_pairing_persists_origin_and_rejects_code_reuse_and_foreign_origin() {
    let server = Server::new().await;
    let n = &server.fixture.node;
    let code = n.new_pair_code()["code"].clone();
    let (mut socket, ready) = server
        .socket(ORIGIN, json!({"type":"pair","code":code}))
        .await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(n.cfg().extension_origin.as_deref(), Some(ORIGIN));
    assert!(n.cfg().public().get("extension_token").is_none());
    let persisted = serialbridge_core::config::Config::load(&n.config_path).unwrap();
    assert_eq!(
        persisted.extension_token.as_deref(),
        ready["token"].as_str()
    );
    socket.close(None).await.unwrap();
    let (_, error) = server
        .socket(ORIGIN, json!({"type":"pair","code":code}))
        .await;
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
    let (_, error) = server
        .socket(
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            json!({"type":"auth","token":ready["token"]}),
        )
        .await;
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
    let (_socket, again) = server
        .socket(ORIGIN, json!({"type":"auth","token":ready["token"]}))
        .await;
    assert_eq!(again["type"], "ready");
}

#[tokio::test]
async fn pairing_expires_and_locks_after_five_bad_attempts() {
    let server = Server::new().await;
    let n = &server.fixture.node;
    let code = n.new_pair_code()["code"].clone();
    n.pairing.lock().unwrap().as_mut().unwrap().expires =
        std::time::Instant::now() - Duration::from_secs(1);
    assert_eq!(
        server
            .socket(ORIGIN, json!({"type":"pair","code":code}))
            .await
            .1["type"],
        "error"
    );
    let code = n.new_pair_code()["code"].clone();
    for _ in 0..5 {
        assert_eq!(
            server
                .socket(ORIGIN, json!({"type":"pair","code":"BAD"}))
                .await
                .1["type"],
            "error"
        );
    }
    assert_eq!(
        server
            .socket(ORIGIN, json!({"type":"pair","code":code}))
            .await
            .1["type"],
        "error"
    );
}

#[tokio::test]
async fn websocket_routes_fetch_and_revoke_fails_pending_tasks_and_credentials() {
    let server = Server::new().await;
    let n = &server.fixture.node;
    n.config.write().unwrap().allowed_origins = vec!["https://example.test".into()];
    let (mut socket, ready) = server.paired().await;
    assert_eq!(ready["type"], "ready");
    let cloned = n.clone();
    let call = tokio::spawn(async move {
        cloned
            .call(
                "local",
                "browser.fetch",
                json!({"url":"https://example.test/api"}),
                Bytes::new(),
                5000,
            )
            .await
    });
    let task = message(&mut socket).await;
    assert_eq!(task["method"], "browser.fetch");
    assert_eq!(task["policy"]["origins"], json!(["https://example.test"]));
    // This is a browser protocol client, not a claim of running Chrome fetch.
    socket.send(Message::Text(json!({"type":"task.result","id":task["id"],"result":{"status":200,"body":"socket-result"}}).to_string().into())).await.unwrap();
    assert_eq!(call.await.unwrap().unwrap().value["body"], "socket-result");
    let mut pending = n
        .begin_browser("browser.fetch", json!({"url":"https://example.test/api"}))
        .await
        .unwrap();
    n.revoke_browser().unwrap();
    assert_eq!(
        pending.next().await.err().unwrap().code,
        "BROWSER_DISCONNECTED"
    );
    let (_, error) = server
        .socket(ORIGIN, json!({"type":"auth","token":ready["token"]}))
        .await;
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn actual_core_accepts_acknowledged_browser_binary_download_and_hashes_it() {
    let server = Server::new().await;
    let n = &server.fixture.node;
    n.config.write().unwrap().allowed_origins = vec!["https://example.test".into()];
    let (mut socket, ready) = server.paired().await;
    assert_eq!(ready["type"], "ready");
    let started=n.start_source_download(json!({"executor":"browser","url":"https://example.test/file","filename":"browser.bin"})).unwrap();
    let id = uuid::Uuid::parse_str(started.value["id"].as_str().unwrap()).unwrap();
    let task = message(&mut socket).await;
    assert_eq!(task["method"], "browser.download");
    let data: Vec<_> = (0..140000).map(|i| (i % 251) as u8).collect();
    socket
        .send(Message::Text(
            json!({"type":"download.begin","id":task["id"],"total_bytes":data.len()})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let mut offset = 0;
    for block in data.chunks(65536) {
        let meta =
            serde_json::to_vec(&json!({"type":"chunk","id":task["id"],"offset":offset})).unwrap();
        let mut binary = (meta.len() as u32).to_be_bytes().to_vec();
        binary.extend(meta);
        binary.extend(block);
        socket.send(Message::Binary(binary.into())).await.unwrap();
        offset += block.len();
        // A completed fetch task may send its best-effort cancellation first.
        let ack = message(&mut socket).await;
        assert_eq!(ack["type"], "chunk.ack");
        assert_eq!(ack["offset"], offset);
    }
    socket
        .send(Message::Text(
            json!({"type":"download.end","id":task["id"],"total_bytes":offset})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let view = completed(n, id).await;
    assert_eq!(view.status, "completed");
    use sha2::{Digest, Sha256};
    assert_eq!(view.sha256.unwrap(), format!("{:x}", Sha256::digest(&data)));
    assert_eq!(
        tokio::fs::read(n.transfers.artifact(id).unwrap().path)
            .await
            .unwrap(),
        data
    );
}
