mod support;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use serialbridge_core::api;
use support::TestNode;
use tower::ServiceExt;
async fn request(
    f: &TestNode,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header(
        "Authorization",
        format!("Bearer {}", f.node.cfg().api_token),
    );
    if body.is_some() {
        req = req.header("Content-Type", "application/json");
    }
    let response = api::router(f.node.clone())
        .oneshot(
            req.body(
                body.map(|v| Body::from(v.to_string()))
                    .unwrap_or_else(Body::empty),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let data = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&data).unwrap())
}
#[tokio::test]
async fn saved_http_port_requires_restart_while_runtime_port_remains_truthful() {
    let f = TestNode::new();
    let actual = f.node.listen_port;
    let (status, value) = request(
        &f,
        "PUT",
        "/v1/settings",
        Some(json!({"node_name":"Saved Name","http_port":9528})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["restart_required"], true);
    assert_eq!(
        request(&f, "GET", "/v1/health", None).await.1["http"]["port"],
        actual
    );
    assert_eq!(
        request(&f, "GET", "/v1/settings", None).await.1["http_port"],
        9528
    );
    assert_eq!(
        serialbridge_core::config::Config::load(&f.node.config_path)
            .unwrap()
            .node_name,
        "Saved Name"
    );
}
#[tokio::test]
async fn rejected_settings_do_not_change_runtime_or_saved_configuration() {
    let f = TestNode::new();
    let before = f.node.cfg();
    for patch in [
        json!({"api_token":"replacement"}),
        json!({"baud_rate":0}),
        json!({"allowed_origins":["https://example.test/path"]}),
    ] {
        assert_eq!(
            request(&f, "PUT", "/v1/settings", Some(patch)).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(f.node.cfg().api_token, before.api_token);
        assert_eq!(f.node.cfg().baud_rate, before.baud_rate);
        assert!(!f.node.config_path.exists());
    }
}
#[tokio::test]
async fn web_console_preflight_allows_only_the_documented_origin() {
    let f = TestNode::new();
    for (origin, expected) in [
        ("http://127.0.0.1:4173", StatusCode::NO_CONTENT),
        ("https://foreign.test", StatusCode::FORBIDDEN),
    ] {
        let response = api::router(f.node.clone())
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/settings")
                    .header("Origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::NO_CONTENT {
            assert_eq!(response.headers()["access-control-allow-origin"], origin);
        }
    }
}
#[tokio::test]
async fn public_rpc_does_not_expose_internal_file_operations() {
    let f = TestNode::new();
    for method in ["shell.exec", "file.read", "file.create"] {
        let (status, error) = request(
            &f,
            "POST",
            "/v1/rpc",
            Some(json!({"method":method,"params":{}})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "METHOD_NOT_FOUND");
    }
}
#[tokio::test]
async fn truncated_http_upload_fails_without_a_completed_artifact() {
    let f = TestNode::new();
    let response = api::router(f.node.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/files/push/truncated.bin")
                .header(
                    "Authorization",
                    format!("Bearer {}", f.node.cfg().api_token),
                )
                .header("Content-Length", "10")
                .body(Body::from("short"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(f.node.transfers.list().is_empty());
    assert_eq!(
        std::fs::read_dir(f.node.cfg().download_dir)
            .unwrap()
            .count(),
        0
    );
}
