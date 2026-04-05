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
        validation::validate_and_normalize_domain,
    },
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    model::SourceHost,
    types::Shared,
};

/// POST /api/sources/:source_id/hosts - Add a host to a manually-managed source
pub async fn create_host<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(source_id): Path<String>,
    Json(req): Json<CreateHostRequest>,
) -> ApiResult<Json<HostResponse>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Verify source exists and is manually managed
    let source = state
        .db
        .get_source(&source_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", source_id)))?;

    if source.url.is_some() || source.path.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot manually add hosts to automatically-managed sources".to_string(),
        ));
    }

    // Validate and normalize domain name
    let normalized_name = validate_and_normalize_domain(&req.name)?;

    let now = Utc::now();
    let host = SourceHost {
        name: normalized_name.clone(),
        source_id: source_id.clone(),
        disposition: req.disposition,
        created_at: now,
        updated_at: now,
    };

    state.db.put_hosts(vec![host.clone()]).await?;
    state.blocklist.reload_host(&host.name).await?;

    Ok(Json(HostResponse {
        name: host.name,
        source_id: host.source_id,
        disposition: host.disposition,
        created_at: host.created_at,
        updated_at: host.updated_at,
    }))
}

/// GET /api/sources/:source_id/hosts - List hosts for a source with pagination
pub async fn list_hosts<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(source_id): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<PaginatedResponse<HostResponse>>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    use crate::api::model::{decode_next_token, encode_next_token};

    // Verify source exists
    state
        .db
        .get_source(&source_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", source_id)))?;

    // Decode next_token to get offset, or start at 0
    let offset = pagination
        .next_token
        .as_ref()
        .and_then(|t| decode_next_token(t))
        .unwrap_or(0);

    // Fetch PAGE_SIZE + 1 to determine if there are more results
    let mut hosts = state
        .db
        .get_hosts_by_source_paginated(&source_id, PAGE_SIZE + 1, offset)
        .await?;

    // Check if there are more results
    let next_token = if hosts.len() > PAGE_SIZE {
        hosts.pop(); // Remove the extra item
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    let items = hosts
        .into_iter()
        .map(|h| HostResponse {
            name: h.name,
            source_id: h.source_id,
            disposition: h.disposition,
            created_at: h.created_at,
            updated_at: h.updated_at,
        })
        .collect();

    Ok(Json(PaginatedResponse { items, next_token }))
}

/// DELETE /api/sources/:source_id/hosts/:name - Delete a host from a manually-managed source
pub async fn delete_host<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path((source_id, name)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Verify source exists and is manually managed
    let source = state
        .db
        .get_source(&source_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", source_id)))?;

    if source.url.is_some() || source.path.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot manually delete hosts from automatically-managed sources".to_string(),
        ));
    }

    state.db.delete_host(&name, &source_id).await?;
    state.blocklist.reload_host(&name).await?;

    Ok(Json(serde_json::json!({ "message": "Host deleted" })))
}
