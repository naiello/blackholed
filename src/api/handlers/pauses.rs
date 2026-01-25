use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use std::net::IpAddr;
use std::str::FromStr;

use crate::{
    api::{
        error::{ApiError, ApiResult},
        model::*,
        state::ApiState,
    },
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    types::Shared,
};

/// POST /api/pause/global - Start a global pause
pub async fn start_global_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Json(req): Json<StartPauseRequest>,
) -> ApiResult<Json<PauseStatusResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Calculate expiration time
    let expires_at = if req.duration_seconds >= 31536000000 {
        // Permanent (>1000 years) - use year 9999
        DateTime::parse_from_rfc3339("9999-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    } else {
        Utc::now() + Duration::seconds(req.duration_seconds)
    };

    // Write to EventStore for persistence
    state
        .eventstore
        .put_global_pause(expires_at)
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_global_pause(true);

    Ok(Json(PauseStatusResponse {
        is_paused: true,
        expires_at: Some(expires_at),
    }))
}

/// DELETE /api/pause/global - Stop the global pause
pub async fn stop_global_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
) -> ApiResult<Json<MessageResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Remove from EventStore
    state
        .eventstore
        .delete_global_pause()
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_global_pause(false);

    Ok(Json(MessageResponse {
        message: "Global pause removed".to_string(),
    }))
}

/// GET /api/pause/global - Get global pause status
pub async fn get_global_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
) -> ApiResult<Json<PauseStatusResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    let pause_expires = state
        .eventstore
        .get_global_pause()
        .await
        .map_err(ApiError::Internal)?;

    let is_paused = pause_expires.is_some_and(|exp| exp > Utc::now());

    Ok(Json(PauseStatusResponse {
        is_paused,
        expires_at: pause_expires,
    }))
}

/// POST /api/pause/clients/:ip - Start a pause for a specific client
pub async fn start_client_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(ip): Path<String>,
    Json(req): Json<StartPauseRequest>,
) -> ApiResult<Json<PauseStatusResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Parse IP address
    let ip_addr = IpAddr::from_str(&ip)
        .map_err(|_| ApiError::BadRequest(format!("Invalid IP address: {}", ip)))?;

    // Calculate expiration time
    let expires_at = if req.duration_seconds >= 31536000000 {
        // Permanent (>1000 years) - use year 9999
        DateTime::parse_from_rfc3339("9999-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    } else {
        Utc::now() + Duration::seconds(req.duration_seconds)
    };

    // Write to EventStore for persistence
    state
        .eventstore
        .put_client_pause(ip_addr, expires_at)
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_client_pause(ip_addr, true).await;

    Ok(Json(PauseStatusResponse {
        is_paused: true,
        expires_at: Some(expires_at),
    }))
}

/// DELETE /api/pause/clients/:ip - Stop the pause for a specific client
pub async fn stop_client_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(ip): Path<String>,
) -> ApiResult<Json<MessageResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Parse IP address
    let ip_addr = IpAddr::from_str(&ip)
        .map_err(|_| ApiError::BadRequest(format!("Invalid IP address: {}", ip)))?;

    // Remove from EventStore
    state
        .eventstore
        .delete_client_pause(ip_addr)
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_client_pause(ip_addr, false).await;

    Ok(Json(MessageResponse {
        message: format!("Pause removed for client {}", ip),
    }))
}

/// GET /api/pause/clients/:ip - Get pause status for a specific client
pub async fn get_client_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(ip): Path<String>,
) -> ApiResult<Json<PauseStatusResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Parse IP address
    let ip_addr = IpAddr::from_str(&ip)
        .map_err(|_| ApiError::BadRequest(format!("Invalid IP address: {}", ip)))?;

    let pause_expires = state
        .eventstore
        .get_client_pause(ip_addr)
        .await
        .map_err(ApiError::Internal)?;

    let is_paused = pause_expires.is_some_and(|exp| exp > Utc::now());

    Ok(Json(PauseStatusResponse {
        is_paused,
        expires_at: pause_expires,
    }))
}
