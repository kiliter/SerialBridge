mod support;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use serde_json::json;
use serialbridge_core::api;
use sha2::{Digest, Sha256};
use support::TestNode;
use tower::ServiceExt;
use uuid::Uuid;

fn id(value: &serde_json::Value) -> Uuid {
    Uuid::parse_str(value["id"].as_str().unwrap()).unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
#[tokio::test]
async fn failed_upload_creation_records_failure_and_releases_capacity() {
    use std::os::unix::fs::PermissionsExt;
    let f = TestNode::new();
    let n = &f.node;
    let path = n.cfg().download_dir;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = n
        .create_upload(json!({"filename":"denied.bin","total_bytes":1}))
        .await;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result.unwrap_err().code, "IO_ERROR");
    assert_eq!(n.transfers.list().first().unwrap().status, "failed");
    n.create_upload(json!({"filename":"available.bin","total_bytes":1}))
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn failed_file_publication_records_failure_and_releases_capacity() {
    let f = TestNode::new();
    let n = &f.node;
    let start = n
        .create_upload(json!({"filename":"missing.bin","total_bytes":0}))
        .await
        .unwrap();
    let tid = id(&start.value);
    tokio::fs::remove_file(n.cfg().download_dir.join(format!("{tid}.part")))
        .await
        .unwrap();
    assert_eq!(
        n.finish_upload(json!({"id":tid})).await.unwrap_err().code,
        "IO_ERROR"
    );
    assert_eq!(n.transfers.get(tid).unwrap().status, "failed");
    assert!(n.transfers.artifact(tid).is_err());
    for name in ["one", "two"] {
        n.create_upload(json!({"filename":name,"total_bytes":0}))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn full_handle_table_does_not_leave_an_untracked_published_upload() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    tokio::fs::write(n.cfg().shared_dir.join("shared.bin"), b"shared")
        .await
        .unwrap();
    for _ in 0..200 {
        n.open_shared(json!({"filename":"shared.bin"}))
            .await
            .unwrap();
    }
    let start = n
        .create_upload(json!({"filename":"upload.bin","total_bytes":0}))
        .await
        .unwrap();
    let tid = id(&start.value);
    assert_eq!(
        n.finish_upload(json!({"id":tid})).await.unwrap_err().code,
        "BUSY"
    );
    assert_eq!(n.transfers.get(tid).unwrap().status, "failed");
    assert!(n.transfers.artifact(tid).is_err());
    assert_eq!(
        std::fs::read_dir(n.cfg().download_dir).unwrap().count(),
        0,
        "发布失败后留下了无法从 API 移除的文件"
    );
    for name in ["one.bin", "two.bin"] {
        n.create_upload(json!({"filename":name,"total_bytes":0}))
            .await
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failed_artifact_deletion_can_be_retried() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = TestNode::new();
    let n = &fixture.node;
    let start = n
        .create_upload(json!({"filename":"retry.bin","total_bytes":0}))
        .await
        .unwrap();
    let tid = id(&start.value);
    n.finish_upload(json!({"id":tid})).await.unwrap();
    let directory = n.cfg().download_dir;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = n.remove_artifact(json!({"id":tid})).await;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert!(
        n.transfers.artifact(tid).is_ok(),
        "删除失败后丢失了文件句柄"
    );
    n.remove_artifact(json!({"id":tid})).await.unwrap();
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

#[tokio::test]
async fn cancelling_an_incoming_upload_updates_status_and_releases_capacity() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    let first = n
        .create_upload(json!({"filename":"cancel.bin","total_bytes":5}))
        .await
        .unwrap();
    let second = n
        .create_upload(json!({"filename":"other.bin","total_bytes":5}))
        .await
        .unwrap();
    n.write_block(
        json!({"id":first.value["id"],"offset":0}),
        Bytes::from_static(b"ab"),
    )
    .await
    .unwrap();
    assert_eq!(
        n.create_upload(json!({"filename":"busy.bin","total_bytes":5}))
            .await
            .unwrap_err()
            .code,
        "BUSY"
    );
    let res = api::router(n.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/downloads/{}", id(&first.value)))
                .header("Authorization", format!("Bearer {}", n.cfg().api_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        n.transfers.get(id(&first.value)).unwrap().status,
        "cancelled"
    );
    assert!(!n
        .cfg()
        .download_dir
        .join(format!("{}.part", id(&first.value)))
        .exists());
    n.create_upload(json!({"filename":"available.bin","total_bytes":5}))
        .await
        .unwrap();
    n.remove_artifact(json!({"id":second.value["id"]}))
        .await
        .unwrap();
}

#[tokio::test]
async fn multichunk_upload_preserves_binary_bytes_and_publishes_only_after_finish() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    let data: Vec<_> = (0..40000).map(|i| (i % 251) as u8).collect();
    let start = n
        .create_upload(
            json!({"filename":"binary.bin","total_bytes":data.len(),"sha256":sha(&data)}),
        )
        .await
        .unwrap();
    let tid = id(&start.value);
    assert!(n.transfers.artifact(tid).is_err());
    let mut offset = 0;
    for block in data.chunks(8192) {
        let ack = n
            .write_block(
                json!({"id":tid,"offset":offset}),
                Bytes::copy_from_slice(block),
            )
            .await
            .unwrap();
        offset += block.len();
        assert_eq!(ack.value["offset"], offset);
    }
    let finished = n.finish_upload(json!({"id":tid})).await.unwrap();
    assert_eq!(finished.value["sha256"], sha(&data));
    assert_eq!(
        tokio::fs::read(n.transfers.artifact(tid).unwrap().path)
            .await
            .unwrap(),
        data
    );
    assert_eq!(n.transfers.get(tid).unwrap().status, "completed");
    n.remove_artifact(json!({"id":tid})).await.unwrap();
    assert!(n.transfers.artifact(tid).is_err());
    assert_eq!(std::fs::read_dir(n.cfg().download_dir).unwrap().count(), 0);
}

#[tokio::test]
async fn wrong_hash_fails_without_publishing_a_file_and_releases_upload_slots() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    let start = n
        .create_upload(json!({"filename":"bad.bin","total_bytes":5,"sha256":sha(b"other")}))
        .await
        .unwrap();
    let tid = id(&start.value);
    n.write_block(json!({"id":tid,"offset":0}), Bytes::from_static(b"hello"))
        .await
        .unwrap();
    assert_eq!(
        n.finish_upload(json!({"id":tid})).await.unwrap_err().code,
        "HASH_MISMATCH"
    );
    assert_eq!(n.transfers.get(tid).unwrap().status, "failed");
    assert!(n.transfers.artifact(tid).is_err());
    assert_eq!(std::fs::read_dir(n.cfg().download_dir).unwrap().count(), 0);
    for name in ["one.bin", "two.bin"] {
        n.create_upload(json!({"filename":name,"total_bytes":0}))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn upload_rejects_file_limit_and_directory_quota() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    {
        let mut cfg = n.config.write().unwrap();
        cfg.max_file_bytes = 10;
        cfg.max_spool_bytes = 15;
    }
    assert_eq!(
        n.create_upload(json!({"filename":"big","total_bytes":11}))
            .await
            .unwrap_err()
            .code,
        "FILE_TOO_LARGE"
    );
    n.create_upload(json!({"filename":"one","total_bytes":10}))
        .await
        .unwrap();
    assert_eq!(
        n.create_upload(json!({"filename":"two","total_bytes":6}))
            .await
            .unwrap_err()
            .code,
        "SPOOL_QUOTA"
    );
}

#[tokio::test]
async fn shared_file_handle_removal_preserves_original_and_rejects_invalid_ranges() {
    let fixture = TestNode::new();
    let n = &fixture.node;
    let path = n.cfg().shared_dir.join("original.bin");
    tokio::fs::write(&path, b"shared").await.unwrap();
    let opened = n
        .open_shared(json!({"filename":"original.bin"}))
        .await
        .unwrap();
    let tid = id(&opened.value);
    for (offset, length) in [(0, 0), (0, 16385), (7, 1), (5, 2)] {
        assert!(n
            .read_block(json!({"id":tid,"offset":offset,"length":length}))
            .await
            .is_err());
    }
    assert_eq!(
        n.read_block(json!({"id":tid,"offset":1,"length":3}))
            .await
            .unwrap()
            .data,
        Bytes::from_static(b"har")
    );
    n.remove_artifact(json!({"id":tid})).await.unwrap();
    assert_eq!(tokio::fs::read(path).await.unwrap(), b"shared");
    assert!(n
        .open_shared(json!({"filename":"../outside"}))
        .await
        .is_err());
}
