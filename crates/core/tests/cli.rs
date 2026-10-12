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
