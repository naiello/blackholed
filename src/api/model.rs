use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::HostDisposition;

#[derive(Deserialize)]
pub struct CreateSourceRequest {
    pub id: String,
    pub url: Option<String>,
    pub path: Option<String>,
    pub disposition: HostDisposition,
}

#[derive(Serialize)]
pub struct SourceResponse {
    pub id: String,
    pub url: Option<String>,
    pub path: Option<String>,
    pub disposition: HostDisposition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateHostRequest {
    pub name: String,
    pub disposition: HostDisposition,
}

#[derive(Serialize)]
pub struct HostResponse {
    pub name: String,
    pub source_id: String,
    pub disposition: HostDisposition,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ClientResponse {
    pub ip: String,
    pub last_seen: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct BlockEventResponse {
    pub domain: String,
    pub time: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct PaginationQuery {
    pub next_token: Option<String>,
}

pub const PAGE_SIZE: usize = 100;

#[derive(Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,
}

/// Encode an offset as an opaque next token
pub fn encode_next_token(offset: usize) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(offset.to_string())
}

/// Decode an opaque next token to an offset
pub fn decode_next_token(token: &str) -> Option<usize> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .ok()?;
    let offset_str = String::from_utf8(decoded).ok()?;
    offset_str.parse().ok()
}
