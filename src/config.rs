use anyhow::Result;
use std::path::PathBuf;
use std::net::{Ipv4Addr, Ipv6Addr};
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
pub struct BlocklistConfig {
    pub url: String,
}

#[derive(PartialEq, Eq, Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hosts_file: PathBuf,
    pub blackhole_address: Ipv4Addr,
    pub blackhole_address_v6: Ipv6Addr,
    pub blackhole_v6: bool,
    pub blocklists: HashMap<String, BlocklistConfig>,
    pub dnsmasq: DnsmasqConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hosts_file: "/var/db/blackholed/hosts".into(),
            blackhole_address: "0.0.0.0".parse().expect("Hardcoded address should parse"),
            blackhole_address_v6: "::1".parse().expect("Hardcoded address should parse"),
            blackhole_v6: true,
            blocklists: HashMap::new(),
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

