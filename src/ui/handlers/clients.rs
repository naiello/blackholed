use std::net::IpAddr;

use anyhow;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    api::{
        error::ApiError,
        model::{encode_next_token, BlockEventResponse, ClientResponse, PaginatedResponse, PAGE_SIZE},
        state::ApiState,
    },
    db::SqlDb,
    eventstore::{EventStore, RedisEventStore},
    ui::{
        handlers::helpers::is_htmx_request,
        templates::{ClientListTemplate, DashboardTemplate, EventListPartial},
    },
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

#[derive(Deserialize)]
pub struct ClientsQuery {
    pub next_token: Option<String>,
}

pub async fn list_clients(
    State(state): State<ConcreteState>,
    Query(query): Query<ClientsQuery>,
) -> Result<Response, ApiError> {
    // Decode pagination token
    let offset = query
        .next_token
        .as_ref()
        .and_then(|t| crate::api::model::decode_next_token(t))
        .unwrap_or(0);

    // Fetch clients from eventstore (fetch PAGE_SIZE + 1 to detect if more exist)
    let mut clients = state
        .eventstore
        .get_clients_paginated(PAGE_SIZE + 1, offset)
        .await
        .map_err(|e| ApiError::Internal(e))?;

    // Determine if there are more results
    let next_token = if clients.len() > PAGE_SIZE {
        clients.pop();
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    // Convert to response models
    let clients = PaginatedResponse {
        items: clients
            .into_iter()
            .map(|c| ClientResponse {
                ip: c.ip.to_string(),
                last_seen: c.last_seen,
            })
            .collect(),
        next_token,
    };

    let template = ClientListTemplate { clients };
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}

#[derive(Deserialize)]
pub struct ClientDetailQuery {
    pub next_token: Option<String>,
}

pub async fn client_detail(
    State(state): State<ConcreteState>,
    Path(ip_str): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ClientDetailQuery>,
) -> Result<Response, ApiError> {
    // Parse IP address
    let ip = ip_str
        .parse::<IpAddr>()
        .map_err(|_| ApiError::BadRequest("Invalid IP address".into()))?;

    // Decode pagination token
    let offset = query
        .next_token
        .as_ref()
        .and_then(|t| crate::api::model::decode_next_token(t))
        .unwrap_or(0);

    // Fetch events from eventstore (fetch PAGE_SIZE + 1 to detect if more exist)
    let mut events = state
        .eventstore
        .get_block_events_for_client_paginated(ip, PAGE_SIZE + 1, offset)
        .await
        .map_err(|e| ApiError::Internal(e))?;

    // Determine if there are more results
    let next_token = if events.len() > PAGE_SIZE {
        events.pop();
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    // Convert to response models
    let events = PaginatedResponse {
        items: events
            .into_iter()
            .map(|e| BlockEventResponse {
                domain: e.name.to_string(),
                time: e.time,
            })
            .collect(),
        next_token,
    };

    let current_ip = ip_str;

    // Return partial or full page based on HTMX
    if is_htmx_request(&headers) {
        let template = EventListPartial { current_ip, events };
        Ok(Html(
            template
                .render()
                .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
        )
        .into_response())
    } else {
        let template = DashboardTemplate { current_ip, events };
        Ok(Html(
            template
                .render()
                .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
        )
        .into_response())
    }
}
