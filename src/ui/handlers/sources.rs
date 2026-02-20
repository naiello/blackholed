use anyhow;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    Form,
};
use chrono::Utc;
use serde::Deserialize;

use crate::{
    api::{
        error::ApiError,
        handlers::sources::source_to_response,
        model::{encode_next_token, HostResponse, PaginatedResponse, PAGE_SIZE},
        state::ApiState,
        validation::{validate_and_normalize_domain, validate_source_id},
    },
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    model::{HostDisposition, Source, SourceHost},
    types::Shared,
    ui::templates::{NewSourceTemplate, SourceDetailTemplate, SourceListTemplate},
};

#[derive(Deserialize)]
pub struct SourcesQuery {
    pub next_token: Option<String>,
}

pub async fn list_sources<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Query(query): Query<SourcesQuery>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Decode pagination token
    let offset = query
        .next_token
        .as_ref()
        .and_then(|t| crate::api::model::decode_next_token(t))
        .unwrap_or(0);

    // Fetch sources from database (fetch PAGE_SIZE + 1 to detect if more exist)
    let mut sources = state
        .db
        .get_all_sources_paginated(PAGE_SIZE + 1, offset)
        .await
        .map_err(ApiError::Internal)?;

    // Determine if there are more results
    let next_token = if sources.len() > PAGE_SIZE {
        sources.pop();
        Some(encode_next_token(offset + PAGE_SIZE))
    } else {
        None
    };

    // Convert to response models
    let sources = PaginatedResponse {
        items: sources.into_iter().map(source_to_response).collect(),
        next_token,
    };

    let template = SourceListTemplate { sources };
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}

pub async fn new_source_form() -> Result<Response, ApiError> {
    let template = NewSourceTemplate { error: None };
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}

#[derive(Deserialize)]
pub struct CreateSourceForm {
    pub id: String,
    pub disposition: String,
    pub url: Option<String>,
}

pub async fn create_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Form(form): Form<CreateSourceForm>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Validate source ID
    validate_source_id(&form.id)?;

    // Reject reserved file- prefix
    if form.id.starts_with("file-") {
        return Err(ApiError::BadRequest(
            "Source IDs starting with 'file-' are reserved for system-managed file sources"
                .to_string(),
        ));
    }

    // Parse disposition
    let disposition: HostDisposition = form
        .disposition
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid disposition".into()))?;

    // Clean up empty strings to None
    let url = form.url.filter(|s| !s.is_empty());

    // Create source
    let now = Utc::now();
    let source = Source {
        id: form.id.clone(),
        url,
        path: None,
        disposition,
        created_at: now,
        updated_at: now,
    };

    state
        .db
        .put_source(source.clone())
        .await
        .map_err(ApiError::Internal)?;

    // If source is auto-managed (has URL), trigger immediate reload
    if source.url.is_some() {
        log::info!("Triggering automatic reload for new source: {}", source.id);
        state
            .sourceloader
            .reload_source(source.id.clone())
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Failed to initiate reload: {}", e)))?;
    }

    // Redirect to source detail
    Ok(Redirect::to(&format!("/sources/{}", form.id)).into_response())
}

#[derive(Deserialize)]
pub struct SourceDetailQuery {
    pub next_token: Option<String>,
}

pub async fn source_detail<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
    Query(query): Query<SourceDetailQuery>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Fetch source
    let source = state
        .db
        .get_source(&id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", id)))?;

    // Convert to response model
    let is_manually_managed = source.url.is_none() && source.path.is_none();
    let source_id = source.id.clone();
    let source_response = source_to_response(source);

    // If this is a manually-managed source, fetch hosts
    let hosts = if is_manually_managed {
        // Decode pagination token
        let offset = query
            .next_token
            .as_ref()
            .and_then(|t| crate::api::model::decode_next_token(t))
            .unwrap_or(0);

        // Fetch hosts (fetch PAGE_SIZE + 1 to detect if more exist)
        let mut hosts = state
            .db
            .get_hosts_by_source_paginated(&source_id, PAGE_SIZE + 1, offset)
            .await
            .map_err(ApiError::Internal)?;

        // Determine if there are more results
        let next_token = if hosts.len() > PAGE_SIZE {
            hosts.pop();
            Some(encode_next_token(offset + PAGE_SIZE))
        } else {
            None
        };

        // Convert to response models
        Some(PaginatedResponse {
            items: hosts
                .into_iter()
                .map(|h| HostResponse {
                    name: h.name,
                    source_id: h.source_id,
                    disposition: h.disposition,
                    created_at: h.created_at,
                    updated_at: h.updated_at,
                })
                .collect(),
            next_token,
        })
    } else {
        None
    };

    let template = SourceDetailTemplate {
        source: source_response,
        hosts,
    };
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}

#[derive(Deserialize)]
pub struct AddHostForm {
    pub name: String,
    pub disposition: String,
}

pub async fn add_host<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(source_id): Path<String>,
    Form(form): Form<AddHostForm>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Verify source exists and is manually-managed
    let source = state
        .db
        .get_source(&source_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", source_id)))?;

    if source.url.is_some() || source.path.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot manually add hosts to automatically-managed source".into(),
        ));
    }

    // Validate and normalize domain
    let normalized = validate_and_normalize_domain(&form.name)?;

    // Parse disposition
    let disposition: HostDisposition = form
        .disposition
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid disposition".into()))?;

    // Create host
    let now = Utc::now();
    let host = SourceHost {
        name: normalized,
        source_id: source_id.clone(),
        disposition,
        created_at: now,
        updated_at: now,
    };

    state
        .db
        .put_hosts(vec![host.clone()])
        .await
        .map_err(ApiError::Internal)?;

    // Reload blocklist
    state
        .blocklist
        .reload_host(&host.name)
        .await
        .map_err(ApiError::Internal)?;

    // Redirect back to source detail
    Ok(Redirect::to(&format!("/sources/{}", source_id)).into_response())
}

pub async fn delete_host<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path((source_id, name)): Path<(String, String)>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    // Verify source exists and is manually-managed
    let source = state
        .db
        .get_source(&source_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source {} not found", source_id)))?;

    if source.url.is_some() || source.path.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot manually delete hosts from automatically-managed source".into(),
        ));
    }

    // Delete host
    state
        .db
        .delete_host(&name, &source_id)
        .await
        .map_err(ApiError::Internal)?;

    // Reload blocklist
    state
        .blocklist
        .reload_host(&name)
        .await
        .map_err(ApiError::Internal)?;

    // Return empty response (HTMX will remove the row)
    Ok((StatusCode::OK, Html("".to_string())).into_response())
}

pub async fn delete_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError>
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
            "Cannot delete file-managed sources. Remove the file from the directory instead."
                .to_string(),
        ));
    }

    // Delete source
    state
        .db
        .delete_source(&id)
        .await
        .map_err(ApiError::Internal)?;

    // Reload blocklist
    state.blocklist.reload().await;

    // Redirect to source list
    Ok(Redirect::to("/sources").into_response())
}

pub async fn reload_source<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError>
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

    // Return success notification
    let notification = r#"<div class="bg-green-100 dark:bg-green-900 border border-green-400 dark:border-green-600 text-green-700 dark:text-green-200 px-4 py-3 rounded relative" role="alert">
            <span class="block sm:inline">Source reload initiated. The list will be updated in the background.</span>
            <script>
                setTimeout(function() {
                    document.getElementById('notification-area').innerHTML = '';
                }, 5000);
            </script>
        </div>"#.to_string();

    Ok((StatusCode::OK, Html(notification)).into_response())
}
