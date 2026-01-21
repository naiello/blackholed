use anyhow;
use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::{api::error::ApiError, ui::templates::HomeTemplate};

pub async fn home() -> Result<Response, ApiError> {
    let template = HomeTemplate {};
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}
