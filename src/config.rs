use anyhow::Result;
use std::path::PathBuf;
use std::collections::HashMap;
use serde::Deserialize;

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DnsmasqConfig {
    pub hup_after_refresh: bool,
    pub pid_file: String,
}

impl Default for DnsmasqConfig {
    fn default() -> Self {
        DnsmasqConfig {
            hup_after_refresh: false,
            pid_file: "/var/run/dnsmasq.pid".into(),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DomainListSource {
    Web { url: String },
    Local { file: String },
}

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hosts_file: PathBuf,
    pub blocklists: HashMap<String, DomainListSource>,
    pub allowlists: HashMap<String, DomainListSource>,
    pub dnsmasq: DnsmasqConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hosts_file: "/var/db/blackholed/hosts".into(),
            blocklists: HashMap::new(),
            allowlists: HashMap::new(),
            dnsmasq: DnsmasqConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let cfg = config::Config::builder()
            .add_source(config::File::with_name("/etc/blackholed/blackholed").required(false))
            .add_source(config::File::with_name(".blackholed").required(false))
            .add_source(config::Environment::with_prefix("BLACKHOLED"))
            .build()?
            .try_deserialize()?;

        Ok(cfg)
    }
}

