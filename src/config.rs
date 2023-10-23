use std::path::PathBuf;
use std::net::Ipv4Addr;

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct BlocklistConfig {
    pub name: String,
    pub url: String,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct Config {
    pub blocklist_storage_dir: PathBuf,
    pub blackhole_address: Ipv4Addr,
    pub blocklists: Vec<BlocklistConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            blocklist_storage_dir: "/var/db/blackholed".into(),
            blackhole_address: "127.0.0.1".parse().expect("Hardcoded address should parse"),
            blocklists: vec![],
        }
    }
}

impl Config {
    pub fn load() -> Self {
        Config::default()
    }
}

