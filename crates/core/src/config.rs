use crate::error::{BridgeError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub node_name: String,
    pub http_port: u16,
    pub serial_port: String,
    pub baud_rate: u32,
    pub api_token: String,
    pub extension_token: Option<String>,
    pub extension_origin: Option<String>,
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub download_dir: PathBuf,
    pub shared_dir: PathBuf,
    pub max_file_bytes: u64,
    pub max_spool_bytes: u64,
    pub chunk_bytes: usize,
    pub auto_reconnect: bool,
    /// TCP forwards: listen locally, the peer connects to `target`.
    pub forwards: Vec<crate::tunnel::Forward>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            node_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "SerialBridge".into()),
            http_port: 9527,
            serial_port: String::new(),
            baud_rate: 115200,
            api_token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            extension_token: None,
            extension_origin: None,
            allowed_origins: vec![],
            allowed_methods: vec!["GET".into(), "HEAD".into()],
            download_dir: "downloads".into(),
            shared_dir: "shared".into(),
            max_file_bytes: 100 * 1024 * 1024,
            max_spool_bytes: 1024 * 1024 * 1024,
            chunk_bytes: 8192,
            auto_reconnect: true,
            forwards: vec![],
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.http_port == 0
            || self.baud_rate < 9600
            || self.baud_rate > 3_000_000
            || !(1024..=16384).contains(&self.chunk_bytes)
        {
            return Err(BridgeError::invalid(
                "Invalid port, baud rate (9600..3000000) or chunk size (1024..16384)",
            ));
        }
        if self.api_token.len() < 32 || self.node_name.is_empty() || self.node_name.len() > 80 {
            return Err(BridgeError::invalid(
                "API token must contain at least 32 characters; node name 1..80 bytes",
            ));
        }
        if self.max_file_bytes == 0
            || self.max_file_bytes > 10 * 1024 * 1024 * 1024
            || self.max_spool_bytes < self.max_file_bytes
        {
            return Err(BridgeError::invalid("Invalid file/spool quota"));
        }
        if self.allowed_origins.len() > 100 {
            return Err(BridgeError::invalid("At most 100 allowed origins"));
        }
        for s in &self.allowed_origins {
            let u = reqwest::Url::parse(s)
                .map_err(|_| BridgeError::invalid("Invalid allowed origin"))?;
            if !matches!(u.scheme(), "http" | "https")
                || !u.username().is_empty()
                || u.password().is_some()
                || u.origin().ascii_serialization() != *s
            {
                return Err(BridgeError::invalid("Use exact origins, e.g. https://example.com (no slash, path, wildcard or credentials)"));
            }
        }
        for m in &self.allowed_methods {
            if !["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].contains(&m.as_str()) {
                return Err(BridgeError::invalid("Invalid HTTP method"));
            }
        }
        crate::tunnel::validate_forwards(&self.forwards)?;
        Ok(())
    }
    pub fn public(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("serializable config");
        let map = value.as_object_mut().expect("config object");
        map.remove("api_token");
        map.remove("extension_token");
        value
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg = if path.exists() {
            toml::from_str::<Config>(&std::fs::read_to_string(path)?).map_err(|error| {
                // TOML 默认错误文本包含原始行，可能将 Token 写入启动日志。
                BridgeError::invalid(format!(
                    "Invalid configuration TOML near byte {}",
                    error.span().map(|span| span.start).unwrap_or(0)
                ))
            })?
        } else {
            Self::default()
        };
        if cfg.api_token.is_empty() {
            cfg.api_token = Self::default().api_token;
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let base = std::fs::canonicalize(parent)?;
        if cfg.download_dir.is_relative() {
            cfg.download_dir = base.join(&cfg.download_dir)
        }
        if cfg.shared_dir.is_relative() {
            cfg.shared_dir = base.join(&cfg.shared_dir)
        }
        cfg.validate()?;
        std::fs::create_dir_all(&cfg.download_dir)?;
        std::fs::create_dir_all(&cfg.shared_dir)?;
        cfg.save(path)?;
        Ok(cfg)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(".config-{}.tmp", Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut f = options.open(&tmp)?;
            use std::io::Write;
            f.write_all(
                toml::to_string_pretty(self)
                    .map_err(BridgeError::io)?
                    .as_bytes(),
            )?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}
pub fn default_path() -> PathBuf {
    std::env::var_os("SERIALBRIDGE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".serialbridge/config.toml")
        })
}
pub fn safe_filename(name: &str) -> Result<String> {
    let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    if name.is_empty()
        || name.len() > 120
        || name == "."
        || name == ".."
        || (name.ends_with('.') || name.ends_with(' '))
        || reserved
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(BridgeError::invalid(
            "Unsafe filename (a plain cross-platform basename is required)",
        ));
    }
    Ok(name.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_configuration_does_not_echo_credentials() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let sentinel = "test-only-private-credential-do-not-log";
        std::fs::write(&path, format!("api_token = \"{sentinel}\" invalid\n")).unwrap();
        let error = Config::load(&path).unwrap_err();
        assert!(
            !error.message.contains(sentinel),
            "配置解析错误泄露了原始凭据行"
        );
    }
    #[test]
    fn failed_save_removes_credential_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();
        assert!(Config::default().save(&path).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
    #[test]
    fn rejects_traversal_and_reserved_names() {
        for s in [
            "../a", "a/b", "a\\b", "CON", "con.txt", "LPT1.zip", "x:", "a.", "a ", "",
        ] {
            assert!(safe_filename(s).is_err(), "{s}")
        }
        assert_eq!(safe_filename("report.zip").unwrap(), "report.zip");
    }
    #[test]
    fn default_configuration_is_deny_by_default() {
        let c = Config::default();
        assert!(c.allowed_origins.is_empty());
        assert!(c.validate().is_ok());
        assert!(c.public().get("api_token").is_none());
    }
    #[test]
    fn exact_origins_only() {
        let mut c = Config {
            allowed_origins: vec!["https://example.com/path".into()],
            ..Config::default()
        };
        assert!(c.validate().is_err());
        c.allowed_origins = vec!["https://example.com".into()];
        assert!(c.validate().is_ok());
    }
}
