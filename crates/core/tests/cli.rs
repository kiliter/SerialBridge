#[test]
fn init_accepts_a_relative_config_filename() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_serialbridge"))
        .args(["--config", "node.toml", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg = serialbridge_core::config::Config::load(&dir.path().join("node.toml")).unwrap();
    assert!(cfg.download_dir.is_absolute());
    assert!(cfg.download_dir.exists());
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&cfg.api_token));
}
#[test]
fn status_never_sends_the_api_token_through_a_proxy() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().unwrap();
    let service = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let cfg = serialbridge_core::config::Config {
        http_port: service.local_addr().unwrap().port(),
        ..Default::default()
    };
    let path = dir.path().join("node.toml");
    cfg.save(&path).unwrap();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = service.accept() {
            let mut buf = [0; 4096];
            let _ = s.read(&mut buf);
            let body = br#"{"ok":true}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(body);
        }
    });
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let mut out = std::process::Command::new(env!("CARGO_BIN_EXE_serialbridge"))
        .args(["--config", path.to_str().unwrap(), "status"])
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // 代理不会应答；回归时 status 会挂起，因此有界等待并结束子进程。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = out.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            let _ = out.kill();
            let _ = out.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(
        proxy.accept().is_err(),
        "status 把携带 API Token 的请求发给了代理"
    );
    assert!(
        status.is_some_and(|s| s.success()),
        "status 未直接访问本机服务"
    );
}
