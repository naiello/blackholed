use anyhow::{Context, Result};
use chrono::TimeDelta;
use hickory_server::resolver::config::NameServerConfigGroup;
use serde::Deserialize;
use std::path::PathBuf;

/// Main configuration structure for blackholed
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub database: DatabaseConfig,
    pub api: ApiConfig,
    pub resolver: ResolverConfig,
    pub eventstore: EventStoreConfig,
    pub sourceloader: SourceLoaderConfig,
    pub blocklist: BlocklistConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            api: ApiConfig::default(),
            resolver: ResolverConfig::default(),
            eventstore: EventStoreConfig::default(),
            sourceloader: SourceLoaderConfig::default(),
            blocklist: BlocklistConfig::default(),
        }
    }
}

/// Database configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/db/blackhole/blackholed.db"),
        }
    }
}

/// API server configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub port: u16,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { port: 5355 }
    }
}

/// DNS resolver configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ResolverConfig {
    pub port: u16,
    pub upstream: UpstreamConfig,
    pub cache_size: usize,
    #[serde(default)]
    pub zones: Vec<ZoneConfig>,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            port: 5353,
            upstream: UpstreamConfig::default(),
            cache_size: 10000,
            zones: Vec::new(),
        }
    }
}

/// Zone configuration for loading zones from files
#[derive(Debug, Clone, Deserialize)]
pub struct ZoneConfig {
    pub name: String,
    pub file: PathBuf,
}

/// Upstream DNS server configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum UpstreamConfig {
    /// Use a well-known DNS provider
    Named { provider: String },
    /// Use custom DNS servers
    Custom { servers: Vec<String> },
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self::Named {
            provider: "cloudflare".to_string(),
        }
    }
}

impl UpstreamConfig {
    pub fn to_nameserver_config_group(&self) -> Result<NameServerConfigGroup> {
        match self {
            UpstreamConfig::Named { provider } => match provider.to_lowercase().as_str() {
                "cloudflare" => Ok(NameServerConfigGroup::cloudflare()),
                "google" => Ok(NameServerConfigGroup::google()),
                "quad9" => Ok(NameServerConfigGroup::quad9()),
                "cloudflare_tls" => Ok(NameServerConfigGroup::cloudflare_tls()),
                "google_tls" => Ok(NameServerConfigGroup::google_tls()),
                "quad9_tls" => Ok(NameServerConfigGroup::quad9_tls()),
                _ => anyhow::bail!("Unknown DNS provider: {}", provider),
            },
            UpstreamConfig::Custom { servers } => {
                anyhow::bail!("Custom DNS servers not yet implemented: {:?}", servers)
            }
        }
    }
}

/// Event store (Redis/Valkey) configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EventStoreConfig {
    pub endpoint: String,
    /// Event TTL in seconds
    #[serde(default = "default_event_ttl_secs")]
    pub event_ttl_secs: i64,
    /// Client TTL in seconds
    #[serde(default = "default_client_ttl_secs")]
    pub client_ttl_secs: i64,
    /// Sweeper interval in seconds
    #[serde(default = "default_sweeper_interval_secs")]
    pub sweeper_interval_secs: i64,
}

fn default_event_ttl_secs() -> i64 {
    3600 // 1 hour
}

fn default_client_ttl_secs() -> i64 {
    86400 // 1 day
}

fn default_sweeper_interval_secs() -> i64 {
    900 // 15 minutes
}

impl Default for EventStoreConfig {
    fn default() -> Self {
        Self {
            endpoint: "redis://127.0.0.1:6379".to_string(),
            event_ttl_secs: default_event_ttl_secs(),
            client_ttl_secs: default_client_ttl_secs(),
            sweeper_interval_secs: default_sweeper_interval_secs(),
        }
    }
}

impl EventStoreConfig {
    pub fn event_ttl(&self) -> TimeDelta {
        TimeDelta::seconds(self.event_ttl_secs)
    }

    pub fn client_ttl(&self) -> TimeDelta {
        TimeDelta::seconds(self.client_ttl_secs)
    }

    pub fn sweeper_interval(&self) -> TimeDelta {
        TimeDelta::seconds(self.sweeper_interval_secs)
    }
}

/// Source loader configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SourceLoaderConfig {
    /// Run interval in seconds
    #[serde(default = "default_run_interval_secs")]
    pub run_interval_secs: i64,
    /// Stale age in seconds
    #[serde(default = "default_stale_age_secs")]
    pub stale_age_secs: i64,
}

fn default_run_interval_secs() -> i64 {
    21600 // 6 hours
}

fn default_stale_age_secs() -> i64 {
    604800 // 7 days
}

impl Default for SourceLoaderConfig {
    fn default() -> Self {
        Self {
            run_interval_secs: default_run_interval_secs(),
            stale_age_secs: default_stale_age_secs(),
        }
    }
}

impl SourceLoaderConfig {
    pub fn run_interval(&self) -> TimeDelta {
        TimeDelta::seconds(self.run_interval_secs)
    }

    pub fn stale_age(&self) -> TimeDelta {
        TimeDelta::seconds(self.stale_age_secs)
    }
}

/// Blocklist configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BlocklistConfig {
    pub wildcard_match: bool,
    pub min_wildcard_depth: u8,
}

impl Default for BlocklistConfig {
    fn default() -> Self {
        Self {
            wildcard_match: true,
            min_wildcard_depth: 2,
        }
    }
}

impl Config {
    /// Load configuration from /etc/blackholed/blackholed.toml and ./blackholed.toml
    /// Falls back to defaults for any missing values
    pub fn load() -> Result<Self> {
        let cfg = config::Config::builder()
            .add_source(
                config::File::with_name("/etc/blackhole/blackholed")
                    .format(config::FileFormat::Toml)
                    .required(false),
            )
            .add_source(
                config::File::with_name("./blackholed")
                    .format(config::FileFormat::Toml)
                    .required(false),
            )
            .build()
            .context("Failed to build configuration")?
            .try_deserialize()
            .context("Failed to deserialize configuration")?;

        Ok(cfg)
    }
}
