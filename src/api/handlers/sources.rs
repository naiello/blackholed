use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;

use crate::{
    api::{
        error::{ApiError, ApiResult},
        model::*,
        state::ApiState,
        validation::{validate_source_id, validate_source_url},
    },
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    model::Source,
    types::Shared,
};

pub fn source_to_response(source: Source) -> SourceResponse {
    let is_file_managed = source.is_file_managed();
    SourceResponse {
        id: source.id,
        url: source.url,
        path: source.path,
        disposition: source.disposition,
        is_file_managed,
        created_at: source.created_at,
        updated_at: source.updated_at,
    }
}

/// POST /api/sources - Create a new source
pub async fn create_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Json(req): Json<CreateSourceRequest>,
) -> ApiResult<Json<SourceResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    validate_source_id(&req.id)?;

    // Reject reserved file- prefix
    if req.id.starts_with("file-") {
        return Err(ApiError::BadRequest(
            "Source IDs starting with 'file-' are reserved for system-managed file sources"
                .to_string(),
        ));
    }

    if let Some(url) = &req.url {
        validate_source_url(url)?;
    } else {
        tracing::info!(source_id = req.id.as_str(), "Creating manually-managed source");
    }

    let now = Utc::now();
    let source = Source {
        id: req.id.clone(),
        url: req.url,
        path: None,
        disposition: req.disposition,
        created_at: now,
        updated_at: now,
    };

    state.db.put_source(source.clone()).await?;

    // If source is auto-managed (has URL), trigger immediate reload
    if source.url.is_some() {
        tracing::info!(source_id = source.id.as_str(), "Triggering automatic reload for new source");
        state
            .sourceloader
            .reload_source(source.id.clone())
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Failed to initiate reload: {}", e)))?;
    }

    Ok(Json(source_to_response(source)))
}

/// GET /api/sources - List all sources with pagination
pub async fn list_sources<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<PaginatedResponse<SourceResponse>>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
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

    let items = sources.into_iter().map(source_to_response).collect();

    Ok(Json(PaginatedResponse { items, next_token }))
}

/// GET /api/sources/:id - Get a specific source
pub async fn get_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
) -> ApiResult<Json<SourceResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    let source = state
        .db
        .get_source(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", id)))?;

    let resp = source_to_response(source);
    Ok(Json(resp))
}

/// DELETE /api/sources/:id - Delete a source
pub async fn delete_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Fetch source to check if it's file-managed
    let source = state
        .db
        .get_source(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", id)))?;

    if source.is_file_managed() {
        return Err(ApiError::BadRequest(
            "Cannot delete file-managed sources via API. Remove the file from the directory instead."
                .to_string(),
        ));
    }

    state.db.delete_source(&id).await?;
    state.blocklist.reload().await;

    Ok(Json(serde_json::json!({ "message": "Source deleted" })))
}

/// POST /api/sources/:id/reload - Trigger manual reload of a source
pub async fn reload_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
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
