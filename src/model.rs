use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use anyhow::bail;
use chrono::{DateTime, Utc};
use hickory_server::proto::rr::{LowerName, RecordType};
use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum HostDisposition {
    Block,
    Allow,
}

impl HostDisposition {
    pub fn as_str(&self) -> &'static str {
        match self {
            HostDisposition::Block => "Block",
            HostDisposition::Allow => "Allow",
        }
    }
}

impl FromStr for HostDisposition {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Block" => Ok(HostDisposition::Block),
            "Allow" => Ok(HostDisposition::Allow),
            _ => bail!("{s} is not a valid HostDisposition"),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct EventStoreClient {
    pub ip: IpAddr,
    pub last_seen: DateTime<Utc>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct EventStoreBlockedEvent {
    pub name: LowerName,
    pub time: DateTime<Utc>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct BlockEvent {
    pub time: DateTime<Utc>,
    pub src: SocketAddr,
    pub name: LowerName,
    pub record_type: RecordType,
}

#[derive(PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub url: Option<String>,
    pub path: Option<String>,
    pub disposition: HostDisposition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct SourceHost {
    pub name: String,
    pub source_id: String,
    pub disposition: HostDisposition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
