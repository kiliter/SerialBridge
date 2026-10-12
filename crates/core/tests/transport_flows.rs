mod support;
use bytes::Bytes;
use serde_json::json;
use std::time::Duration;
use support::Pair;

#[tokio::test]
async fn bidirectional_binary_writes_progress_with_small_transport_buffers() {
    let pair = Pair::new(1024).await;
    let a = &pair.a.node;
    let b = &pair.b.node;
    let (ua, ub) = tokio::join!(
        a.peer_call(
            "file.create",
            json!({"filename":"a.bin","total_bytes":8192}),
            Bytes::new(),
            5000
        ),
        b.peer_call(
            "file.create",
            json!({"filename":"b.bin","total_bytes":8192}),
            Bytes::new(),
            5000
        )
    );
    let aid = ua.unwrap().value["id"].clone();
    let bid = ub.unwrap().value["id"].clone();
    let writes = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            a.peer_call(
                "file.write",
                json!({"id":aid,"offset":0}),
                Bytes::from(vec![0xab; 8192]),
                5000
            ),
            b.peer_call(
                "file.write",
                json!({"id":bid,"offset":0}),
                Bytes::from(vec![0xcd; 8192]),
                5000
            )
        )
    })
    .await
    .expect("both nodes must read while writing a frame");
    assert_eq!(writes.0.unwrap().value["offset"], 8192);
    assert_eq!(writes.1.unwrap().value["offset"], 8192);
}

#[tokio::test]
async fn disconnected_nodes_fail_pending_requests_and_clear_peer_state() {
    let mut pair = Pair::new(1024).await;
    // A registered waiter represents a sent request whose result has not arrived.
    let epoch = pair.a.node.peer.lock().unwrap().as_ref().unwrap().epoch;
    let (tx, rx) = tokio::sync::oneshot::channel();
    pair.a.node.pending.lock().unwrap().insert(
        uuid::Uuid::new_v4(),
        serialbridge_core::node::PeerPending { epoch, tx },
    );
    pair.disconnect().await;
    assert!(rx.await.is_err());
    assert!(pair.a.node.pending.lock().unwrap().is_empty());
    assert_eq!(pair.a.node.snapshot()["peer"]["connected"], false);
    assert_eq!(pair.a.node.snapshot()["peer"]["browser"], false);
    let err = pair
        .a
        .node
        .peer_call("system.ping", json!({}), Bytes::new(), 1000)
        .await
        .unwrap_err();
    assert_eq!(err.code, "PEER_DISCONNECTED");
}
