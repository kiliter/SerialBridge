mod support;
use axum::{body::Body, routing::get, Router};
use bytes::Bytes;
use serde_json::json;
use serialbridge_core::node::Node;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::{Pair, TestNode};
struct Fixture {
    origin: String,
    count: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let slow_count = count.clone();
        let router = Router::new()
            .route("/binary", get(|| async { vec![0_u8, 255, 128, 1] }))
            .route(
                "/redirect",
                get(|| async { axum::response::Redirect::temporary("/binary") }),
            )
            .route(
                "/large",
                get(|| async {
                    Body::from_stream(futures_util::stream::iter([
                        Ok::<_, std::io::Error>(Bytes::from(vec![0; 10000])),
                        Ok(Bytes::from(vec![0; 10000])),
                    ]))
                }),
            )
            .route(
                "/slow",
                get(move || {
                    let count = slow_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        "late"
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            origin,
            count,
            task,
        }
    }
    fn allow(&self, node: &Arc<Node>) {
        node.config.write().unwrap().allowed_origins = vec![self.origin.clone()];
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[tokio::test]
async fn real_http_binary_response_uses_lossless_base64() {
    let web = Fixture::new().await;
    let n = TestNode::new();
    web.allow(&n.node);
    let r = n
        .node
        .call(
            "local",
            "http.fetch",
            json!({"url":format!("{}/binary",web.origin)}),
            Bytes::new(),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(r.value["status"], 200);
    assert_eq!(r.value["body_encoding"], "base64");
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(r.value["body_base64"].as_str().unwrap())
            .unwrap(),
        [0, 255, 128, 1]
    );
}
#[tokio::test]
async fn actual_stream_without_content_length_is_bounded() {
    let web = Fixture::new().await;
    let n = TestNode::new();
    web.allow(&n.node);
    let err = n
        .node
        .call(
            "local",
            "http.fetch",
            json!({"url":format!("{}/large",web.origin)}),
            Bytes::new(),
            5000,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "RESPONSE_TOO_LARGE");
}
#[tokio::test]
async fn actual_http_redirect_is_rejected() {
    let web = Fixture::new().await;
    let n = TestNode::new();
    web.allow(&n.node);
    assert_eq!(
        n.node
            .call(
                "local",
                "http.fetch",
                json!({"url":format!("{}/redirect",web.origin)}),
                Bytes::new(),
                5000
            )
            .await
            .unwrap_err()
            .code,
        "REDIRECT_NOT_ALLOWED"
    );
}
#[tokio::test]
async fn peer_timeout_cleans_pending_requests_and_does_not_replay_http() {
    let web = Fixture::new().await;
    let pair = Pair::new(1024).await;
    web.allow(&pair.b.node);
    assert_eq!(
        pair.a
            .node
            .call(
                "peer",
                "http.fetch",
                json!({"url":format!("{}/slow",web.origin)}),
                Bytes::new(),
                1000
            )
            .await
            .unwrap_err()
            .code,
        "TIMEOUT"
    );
    assert!(pair.a.node.pending.lock().unwrap().is_empty());
    assert_eq!(
        pair.a
            .node
            .call("peer", "system.ping", json!({}), Bytes::new(), 1000)
            .await
            .unwrap()
            .value["protocol"],
        1
    );
    assert_eq!(web.count.load(Ordering::SeqCst), 1);
}
