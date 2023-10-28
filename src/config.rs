use anyhow::Result;
use std::path::PathBuf;
use std::net::Ipv4Addr;
use std::collections::HashMap;
use serde::Deserialize;

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct BlocklistConfig {
    pub url: String,
}

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hosts_file: PathBuf,
    pub blackhole_address: Ipv4Addr,
    pub blocklists: HashMap<String, BlocklistConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hosts_file: "hosts.blocked".into(),
            blackhole_address: "0.0.0.0".parse().expect("Hardcoded address should parse"),
            blocklists: HashMap::new(),
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

