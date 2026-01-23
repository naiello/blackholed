use axum::{
    extract::{Path, Query, State},
    Json,
};
use std::net::IpAddr;
use std::str::FromStr;

use crate::{
    api::{
        error::{ApiError, ApiResult},
        model::*,
        state::ApiState,
    },
    db::SqlDb,
    eventstore::{EventStore, RedisEventStore},
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

/// GET /api/clients - List all active clients with pagination
pub async fn list_clients(
    State(state): State<ConcreteState>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<PaginatedResponse<ClientResponse>>> {
    use crate::api::model::{decode_next_token, encode_next_token};

    // Decode next_token to get offset, or start at 0
    let offset = pagination
        .next_token
        .as_ref()
        .and_then(|t| decode_next_token(t))
        .unwrap_or(0);

    // Fetch PAGE_SIZE + 1 to determine if there are more results
    let mut clients = state
        .eventstore
        .get_clients_paginated(PAGE_SIZE + 1, offset)
        .await?;

    // Check if there are more results
    let next_token = if clients.len() > PAGE_SIZE {
        clients.pop(); // Remove the extra item
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    let items: Vec<ClientResponse> = clients
        .into_iter()
        .map(|c| ClientResponse {
            ip: c.ip.to_string(),
            last_seen: c.last_seen,
            is_paused: None,
            pause_expires: None,
        })
        .collect();

    Ok(Json(PaginatedResponse { items, next_token }))
}

/// GET /api/clients/:ip/events - List block events for a specific client with pagination
pub async fn list_client_events(
    State(state): State<ConcreteState>,
    Path(ip): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<PaginatedResponse<BlockEventResponse>>> {
    use crate::api::model::{decode_next_token, encode_next_token};

    let ip_addr = IpAddr::from_str(&ip)
        .map_err(|_| ApiError::BadRequest(format!("Invalid IP address: {}", ip)))?;

    // Decode next_token to get offset, or start at 0
    let offset = pagination
        .next_token
        .as_ref()
        .and_then(|t| decode_next_token(t))
        .unwrap_or(0);

    // Fetch PAGE_SIZE + 1 to determine if there are more results
    let mut events = state
        .eventstore
        .get_block_events_for_client_paginated(ip_addr, PAGE_SIZE + 1, offset)
        .await?;

    // Check if there are more results
    let next_token = if events.len() > PAGE_SIZE {
        events.pop(); // Remove the extra item
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    let items: Vec<BlockEventResponse> = events
        .into_iter()
        .map(|e| BlockEventResponse {
            domain: e.name.to_string(),
            time: e.time,
        })
        .collect();

    Ok(Json(PaginatedResponse { items, next_token }))
}
