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
    pub hosts_file: PathBuf,
    pub blackhole_address: Ipv4Addr,
    pub blocklists: Vec<BlocklistConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            blocklist_storage_dir: "/var/db/blackholed".into(),
            hosts_file: "hosts.blocked".into(),
            blackhole_address: "0.0.0.0".parse().expect("Hardcoded address should parse"),
            blocklists: vec![
                BlocklistConfig { name: "adaway".into(), url: "https://adaway.org/hosts.txt".into()},
                BlocklistConfig { name: "adguard-dns".into(), url: "https://v.firebog.net/hosts/AdguardDNS.txt".into() }
            ],
        }
    }
}

impl Config {
    pub fn load() -> Self {
        Config::default()
    }
}

