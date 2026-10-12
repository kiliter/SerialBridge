use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use serde_json::json;
use serialbridge_core::{api, config::Config, node::Node, transport};
use std::time::Duration;
use tower::ServiceExt;
fn node() -> (tempfile::TempDir, std::sync::Arc<Node>) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        download_dir: dir.path().into(),
        shared_dir: dir.path().into(),
        ..Config::default()
    };
    let n = Node::new(cfg, dir.path().join("config.toml")).unwrap();
    (dir, n)
}
#[tokio::test]
async fn private_api_requires_token() {
    let (_dir, n) = node();
    let res = api::router(n.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = api::router(n.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .header("Authorization", format!("Bearer {}", n.cfg().api_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
#[tokio::test]
async fn foreign_web_origin_is_rejected_even_with_token() {
    let (_dir, n) = node();
    let res = api::router(n.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .header("Authorization", format!("Bearer {}", n.cfg().api_token))
                .header("Origin", "https://evil.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn nodes_can_call_each_other_concurrently_over_the_same_stream() {
    let (_a, a) = node();
    let (_b, b) = node();
    let (left, right) = tokio::io::duplex(512 * 1024);
    let aa = a.clone();
    let bb = b.clone();
    let ha = tokio::spawn(async move { transport::session(aa, left).await });
    let hb = tokio::spawn(async move { transport::session(bb, right).await });
    for _ in 0..100 {
        if a.snapshot()["peer"]["connected"] == true && b.snapshot()["peer"]["connected"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (ra, rb) = tokio::join!(
        a.call("peer", "system.ping", json!({}), Bytes::new(), 5000),
        b.call("peer", "system.ping", json!({}), Bytes::new(), 5000)
    );
    assert!(ra.is_ok());
    assert!(rb.is_ok());
    ha.abort();
    let _ = ha.await;
    hb.abort();
    let _ = hb.await;
    assert_eq!(a.snapshot()["peer"]["connected"], false);
    assert_eq!(a.snapshot()["peer"]["browser"], false);
}
#[tokio::test]
async fn unknown_method_is_explicit_not_a_placeholder_success() {
    let (_d, n) = node();
    assert_eq!(
        n.execute_local("shell.exec".into(), json!({}), Bytes::new())
            .await
            .unwrap_err()
            .code,
        "METHOD_NOT_FOUND"
    );
}
