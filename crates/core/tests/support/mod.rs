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
/// Forwards bytes and flips one random bit with probability 1/`every` per byte,
/// emulating random line noise on a real serial cable (deterministic seed).
async fn noisy_copy<R, W>(mut from: R, mut to: W, every: u64, mut rng: u64)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 4096];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for byte in &mut buf[..n] {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            if rng.is_multiple_of(every) {
                *byte ^= 1 << ((rng >> 32) % 8);
            }
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

impl Pair {
    pub async fn new(capacity: usize) -> Self {
        Self::with_noise(capacity, 0).await
    }
    /// Links two nodes; `every` > 0 corrupts on average one bit per `every`
    /// bytes in each direction.
    pub async fn with_noise(capacity: usize, every: u64) -> Self {
        let a = TestNode::new();
        let b = TestNode::new();
        let (left, right) = if every == 0 {
            tokio::io::duplex(capacity)
        } else {
            let (left, la) = tokio::io::duplex(capacity);
            let (right, rb) = tokio::io::duplex(capacity);
            let (ar, aw) = tokio::io::split(la);
            let (br, bw) = tokio::io::split(rb);
            tokio::spawn(noisy_copy(ar, bw, every, 0x9E37_79B9_7F4A_7C15));
            tokio::spawn(noisy_copy(br, aw, every, 0xD1B5_4A32_D192_ED03));
            (left, right)
        };
        let aa = a.node.clone();
        let bb = b.node.clone();
        let tasks = vec![
            tokio::spawn(async move { transport::session(aa, left).await }),
            tokio::spawn(async move { transport::session(bb, right).await }),
        ];
        tokio::time::timeout(Duration::from_secs(5), async {
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
