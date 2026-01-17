use chrono::{DateTime, Utc};
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

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Block" => Some(HostDisposition::Block),
            "Allow" => Some(HostDisposition::Allow),
            _ => None,
        }
    }
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
