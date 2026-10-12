#![allow(dead_code)]
use serialbridge_core::{config::Config, node::Node, transport};
use std::{sync::Arc, time::Duration};

pub struct TestNode {
    pub root: tempfile::TempDir,
    pub node: Arc<Node>,
}
impl TestNode {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config {
            download_dir: root.path().join("downloads"),
            shared_dir: root.path().join("shared"),
            ..Config::default()
        };
        std::fs::create_dir_all(&cfg.download_dir).unwrap();
        std::fs::create_dir_all(&cfg.shared_dir).unwrap();
        let node = Node::new(cfg, root.path().join("config.toml")).unwrap();
        Self { root, node }
    }
}
pub struct Pair {
    pub a: TestNode,
    pub b: TestNode,
    tasks: Vec<tokio::task::JoinHandle<serialbridge_core::error::Result<()>>>,
}
impl Pair {
    pub async fn new(capacity: usize) -> Self {
        let a = TestNode::new();
        let b = TestNode::new();
        let (left, right) = tokio::io::duplex(capacity);
        let aa = a.node.clone();
        let bb = b.node.clone();
        let tasks = vec![
            tokio::spawn(async move { transport::session(aa, left).await }),
            tokio::spawn(async move { transport::session(bb, right).await }),
        ];
        tokio::time::timeout(Duration::from_secs(2), async {
            while a.node.snapshot()["peer"]["connected"] != true
                || b.node.snapshot()["peer"]["connected"] != true
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("peer handshake");
        Self { a, b, tasks }
    }
    pub async fn disconnect(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Pair {
    fn drop(&mut self) {
        self.a.node.shutdown.cancel();
        self.b.node.shutdown.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}
