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

#[tokio::test]
async fn peer_disconnect_releases_its_upload_but_preserves_local_upload() {
    let mut pair = Pair::new(1024).await;
    let local = pair
        .b
        .node
        .create_upload(json!({"filename":"local.bin","total_bytes":10,"local_staging":true}))
        .await
        .unwrap();
    let remote = pair
        .a
        .node
        .peer_call(
            "file.create",
            json!({"filename":"peer.bin","total_bytes":10}),
            Bytes::new(),
            5000,
        )
        .await
        .unwrap();
    let remote_id = uuid::Uuid::parse_str(remote.value["id"].as_str().unwrap()).unwrap();
    let local_id = uuid::Uuid::parse_str(local.value["id"].as_str().unwrap()).unwrap();
    let original = pair.b.node.cfg().shared_dir.join("shared.bin");
    tokio::fs::write(&original, b"shared").await.unwrap();
    let peer_handle = pair
        .a
        .node
        .peer_call(
            "file.open",
            json!({"filename":"shared.bin"}),
            Bytes::new(),
            5000,
        )
        .await
        .unwrap();
    let local_handle = pair
        .b
        .node
        .open_shared(json!({"filename":"shared.bin"}))
        .await
        .unwrap();
    pair.disconnect().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while pair.b.node.transfers.get(remote_id).unwrap().status != "cancelled" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("peer upload must be cancelled at disconnect");
    assert_eq!(
        pair.b.node.transfers.get(local_id).unwrap().status,
        "running"
    );
    assert!(pair
        .b
        .node
        .transfers
        .artifact(uuid::Uuid::parse_str(peer_handle.value["id"].as_str().unwrap()).unwrap())
        .is_err());
    assert!(pair
        .b
        .node
        .transfers
        .artifact(uuid::Uuid::parse_str(local_handle.value["id"].as_str().unwrap()).unwrap())
        .is_ok());
    assert!(original.exists());
    tokio::time::timeout(Duration::from_secs(1), async {
        while pair
            .b
            .node
            .cfg()
            .download_dir
            .join(format!("{remote_id}.part"))
            .exists()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("peer partial file cleanup");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if pair
                .b
                .node
                .create_upload(json!({"filename":"available.bin","total_bytes":0}))
                .await
                .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("upload capacity released after asynchronous disconnect cleanup");
}
