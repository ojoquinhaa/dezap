use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use config::{Config, Environment, File};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Application configuration merged from defaults, config files, and CLI overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub listen: ListenConfig,
    pub peer: PeerConfig,
    pub identity: IdentityConfig,
    pub paths: PathsConfig,
    pub limits: LimitsConfig,
    pub tls: TlsConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
    pub discovery: DiscoveryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen: ListenConfig::default(),
            peer: PeerConfig::default(),
            identity: IdentityConfig::default(),
            paths: PathsConfig::default(),
            limits: LimitsConfig::default(),
            tls: TlsConfig::default(),
            ui: UiConfig::default(),
            logging: LoggingConfig::default(),
            discovery: DiscoveryConfig::default(),
        }
    }
}

impl AppConfig {
    /// Loads configuration using defaults, optionally merging explicit files.
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let mut builder = Config::builder();

        if let Some(default_path) = Self::default_config_path() {
            builder = builder.add_source(File::from(default_path).required(false));
        }

        if let Some(path) = explicit_path {
            builder = builder.add_source(File::from(path).required(true));
        }

        builder = builder.add_source(Environment::with_prefix("DEZAP").separator("__"));

        let settings = builder
            .build()
            .context("failed to build configuration values")?;

        let mut cfg: AppConfig = settings
            .try_deserialize()
            .context("failed to deserialize configuration")?;

        cfg.paths.normalize()?;
        Ok(cfg)
    }

    fn default_config_path() -> Option<PathBuf> {
        ProjectDirs::from("io", "dezap", "Dezap").map(|dirs| dirs.config_dir().join("config.toml"))
    }
}

/// Listener related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenConfig {
    pub bind_addr: SocketAddr,
    pub password: Option<String>,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 5000),
            password: None,
        }
    }
}

/// Default peer configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerConfig {
    pub default_peer: Option<SocketAddr>,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self { default_peer: None }
    }
}

/// Local identity preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentityConfig {
    pub username: String,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            username: "dezapster".to_string(),
        }
    }
}

/// Filesystem locations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub download_dir: PathBuf,
    pub chat_log: Option<PathBuf>,
    pub history_dir: PathBuf,
    pub peers_file: PathBuf,
}

impl PathsConfig {
    /// Expands tilde paths and ensures target directories exist.
    pub fn normalize(&mut self) -> Result<()> {
        self.download_dir = Self::expand_path(&self.download_dir);
        if let Some(chat) = &mut self.chat_log {
            *chat = Self::expand_path(chat);
            if let Some(parent) = chat.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create chat log directory {parent:?}"))?;
            }
        }
        fs::create_dir_all(&self.download_dir).with_context(|| {
            format!(
                "failed to create download directory {}",
                self.download_dir.display()
            )
        })?;
        self.history_dir = Self::expand_path(&self.history_dir);
        fs::create_dir_all(&self.history_dir).with_context(|| {
            format!(
                "failed to create history directory {}",
                self.history_dir.display()
            )
        })?;
        self.peers_file = Self::expand_path(&self.peers_file);
        if let Some(parent) = self.peers_file.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create peers file directory {}", parent.display())
            })?;
        }
        Ok(())
    }

    fn expand_path(path: &Path) -> PathBuf {
        PathBuf::from(shellexpand::tilde(path.to_string_lossy().as_ref()).into_owned())
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        let download = ProjectDirs::from("io", "dezap", "Dezap")
            .map(|dirs| dirs.data_dir().join("downloads"))
            .unwrap_or_else(|| PathBuf::from("./downloads"));
        let base = ProjectDirs::from("io", "dezap", "Dezap");
        let chat_log = base.as_ref().map(|dirs| dirs.data_dir().join("chat.log"));
        let history_dir = base
            .as_ref()
            .map(|dirs| dirs.data_dir().join("history"))
            .unwrap_or_else(|| PathBuf::from("./history"));
        let peers_file = base
            .as_ref()
            .map(|dirs| dirs.config_dir().join("peers.json"))
            .unwrap_or_else(|| PathBuf::from("./peers.json"));
        Self {
            download_dir: download,
            chat_log,
            history_dir,
            peers_file,
        }
    }
}

/// Limits that protect resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    pub max_message_bytes: usize,
    pub max_file_bytes: u64,
    pub chunk_size_bytes: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: 16 * 1024,
            max_file_bytes: 1 * 1024 * 1024 * 1024,
            chunk_size_bytes: 64 * 1024,
        }
    }
}

/// TLS configuration for QUIC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub insecure_local: bool,
    pub server_name: String,
}

impl TlsConfig {
    pub fn server_name(&self) -> &str {
        self.server_name.as_str()
    }
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert_path: None,
            key_path: None,
            insecure_local: true,
            server_name: "dezap.local".to_string(),
        }
    }
}

/// UI preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub show_timestamps: bool,
    pub accent: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_timestamps: true,
            accent: "crimson".to_string(),
        }
    }
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl LoggingConfig {
    pub fn level(&self) -> &str {
        self.level.as_str()
    }
}

/// Peer discovery settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub port: u16,
    pub response_ttl_ms: u64,
    pub magic: String,
    pub broadcast: Option<Ipv4Addr>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 54095,
            response_ttl_ms: 2_000,
            magic: "dezap-discovery".to_string(),
            broadcast: None,
        }
    }
}
