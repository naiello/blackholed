use std::net::{IpAddr, SocketAddr};

use anyhow;
use askama::Template;
use axum::{
    extract::{ConnectInfo, Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
    Form,
};
use chrono::Utc;
use serde::Deserialize;

use crate::{
    api::{
        error::ApiError,
        model::{encode_next_token, BlockEventResponse, PaginatedResponse, PAGE_SIZE},
        state::ApiState,
        validation::validate_and_normalize_domain,
    },
    db::{Db, SqlDb},
    eventstore::{EventStore, RedisEventStore},
    model::{HostDisposition, SourceHost},
    ui::{
        handlers::helpers::{extract_client_ip, is_htmx_request},
        templates::{ClientPauseInfo, DashboardTemplate, EventListPartial, GlobalPauseInfo},
    },
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub ip: Option<String>,
    pub next_token: Option<String>,
}

pub async fn dashboard(
    State(state): State<ConcreteState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<DashboardQuery>,
) -> Result<Response, ApiError> {
    // Determine which IP to show events for
    let client_ip = if let Some(ip_str) = query.ip {
        ip_str
            .parse::<IpAddr>()
            .map_err(|_| ApiError::BadRequest("Invalid IP address".into()))?
    } else {
        extract_client_ip(ConnectInfo(addr), &headers)
    };

    // Decode pagination token
    let offset = query
        .next_token
        .as_ref()
        .and_then(|t| crate::api::model::decode_next_token(t))
        .unwrap_or(0);

    // Fetch events from eventstore (fetch PAGE_SIZE + 1 to detect if more exist)
    let mut events = state
        .eventstore
        .get_block_events_for_client_paginated(client_ip, PAGE_SIZE + 1, offset)
        .await
        .map_err(ApiError::Internal)?;

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

    let current_ip = client_ip.to_string();

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
        // Fetch global pause status
        let global_pause = state
            .eventstore
            .get_global_pause()
            .await
            .map_err(ApiError::Internal)?;

        let global_pause_info = GlobalPauseInfo {
            is_paused: global_pause.is_some_and(|exp| exp > Utc::now()),
            expires_at: global_pause,
        };

        // Fetch client-specific pause status
        let client_pause_exp = state
            .eventstore
            .get_client_pause(client_ip)
            .await
            .map_err(ApiError::Internal)?;

        let client_pause_info = Some(ClientPauseInfo {
            is_paused: client_pause_exp.is_some_and(|exp| exp > Utc::now()),
            expires_at: client_pause_exp,
            ip: client_ip.to_string(),
        });

        let template = DashboardTemplate {
            current_ip,
            events,
            global_pause: global_pause_info,
            client_pause: client_pause_info,
        };
        Ok(Html(
            template
                .render()
                .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
        )
        .into_response())
    }
}

#[derive(Deserialize)]
pub struct AllowlistRequest {
    pub domain: String,
}

pub async fn allowlist_domain(
    State(state): State<ConcreteState>,
    Form(req): Form<AllowlistRequest>,
) -> Result<Html<String>, ApiError> {
    // Validate and normalize domain
    let normalized = validate_and_normalize_domain(&req.domain)?;

    // Add to webmanaged source
    let now = Utc::now();
    let host = SourceHost {
        name: normalized.clone(),
        source_id: "webmanaged".to_string(),
        disposition: HostDisposition::Allow,
        created_at: now,
        updated_at: now,
    };

    state
        .db
        .put_hosts(vec![host])
        .await
        .map_err(ApiError::Internal)?;

    // Reload blocklist
    state
        .blocklist
        .reload_host(&normalized)
        .await
        .map_err(ApiError::Internal)?;

    // Return success message (will replace the event row)
    let html = format!(
        r#"<div class="p-4 border border-green-200 dark:border-green-800 rounded bg-green-50 dark:bg-green-900/20">
            <p class="text-green-800 dark:text-green-200">
                <strong class="font-mono">{}</strong> has been allowlisted permanently
            </p>
        </div>"#,
        normalized
    );

    Ok(Html(html))
}
