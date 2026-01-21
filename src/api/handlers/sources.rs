use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;

use crate::{
    api::{
        error::{ApiError, ApiResult},
        model::*,
        state::ApiState,
        validation::validate_source_id,
    },
    db::{Db, SqlDb},
    eventstore::RedisEventStore,
    model::Source,
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

/// POST /api/sources - Create a new source
#[axum::debug_handler]
pub async fn create_source(
    State(state): State<ConcreteState>,
    Json(req): Json<CreateSourceRequest>,
) -> ApiResult<Json<SourceResponse>> {
    validate_source_id(&req.id)?;

    if req.url.is_none() && req.path.is_none() {
        log::info!("Creating manually-managed source: {}", req.id);
    }

    let now = Utc::now();
    let source = Source {
        id: req.id.clone(),
        url: req.url,
        path: req.path,
        disposition: req.disposition,
        created_at: now,
        updated_at: now,
    };

    state.db.put_source(source.clone()).await?;

    // If source is auto-managed (has URL or path), trigger immediate reload
    if source.url.is_some() || source.path.is_some() {
        log::info!("Triggering automatic reload for new source: {}", source.id);
        state
            .sourceloader
            .reload_source(source.id.clone())
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Failed to initiate reload: {}", e)))?;
        // Note: blocklist will be reloaded automatically by the sourceloader after successful fetch
    }
    // Note: No need to reload blocklist for manually-managed sources - they're empty on creation

    Ok(Json(SourceResponse {
        id: source.id,
        url: source.url,
        path: source.path,
        disposition: source.disposition,
        created_at: source.created_at,
        updated_at: source.updated_at,
    }))
}

/// GET /api/sources - List all sources with pagination
pub async fn list_sources(
    State(state): State<ConcreteState>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<PaginatedResponse<SourceResponse>>> {
    use crate::api::model::{decode_next_token, encode_next_token};

    // Decode next_token to get offset, or start at 0
    let offset = pagination
        .next_token
        .as_ref()
        .and_then(|t| decode_next_token(t))
        .unwrap_or(0);

    // Fetch PAGE_SIZE + 1 to determine if there are more results
    let mut sources = state
        .db
        .get_all_sources_paginated(PAGE_SIZE + 1, offset)
        .await?;

    // Check if there are more results
    let next_token = if sources.len() > PAGE_SIZE {
        sources.pop(); // Remove the extra item
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    let items = sources
        .into_iter()
        .map(|s| SourceResponse {
            id: s.id,
            url: s.url,
            path: s.path,
            disposition: s.disposition,
            created_at: s.created_at,
            updated_at: s.updated_at,
        })
        .collect();

    Ok(Json(PaginatedResponse { items, next_token }))
}

/// GET /api/sources/:id - Get a specific source
pub async fn get_source(
    State(state): State<ConcreteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SourceResponse>> {
    let source = state
        .db
        .get_source(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", id)))?;

    Ok(Json(SourceResponse {
        id: source.id,
        url: source.url,
        path: source.path,
        disposition: source.disposition,
        created_at: source.created_at,
        updated_at: source.updated_at,
    }))
}

/// DELETE /api/sources/:id - Delete a source
pub async fn delete_source(
    State(state): State<ConcreteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.db.delete_source(&id).await?;
    state.blocklist.reload().await;

    Ok(Json(serde_json::json!({ "message": "Source deleted" })))
}

/// POST /api/sources/:id/reload - Trigger manual reload of a source
pub async fn reload_source(
    State(state): State<ConcreteState>,
    Path(id): Path<String>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    // Verify source exists
    let source = state
        .db
        .get_source(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", id)))?;

    // Verify source is auto-managed (has URL or path)
    if source.url.is_none() && source.path.is_none() {
        return Err(ApiError::BadRequest(
            "Cannot reload manually-managed source (no URL or path defined)".to_string(),
        ));
    }

    // Trigger reload asynchronously (non-blocking)
    state
        .sourceloader
        .reload_source(id.clone())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("Failed to initiate reload: {}", e)))?;

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "message": "Source reload initiated",
            "source_id": id
        })),
    ))
}
