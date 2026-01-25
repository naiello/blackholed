use anyhow;
use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use chrono::Utc;

use crate::{
    api::{error::ApiError, state::ApiState},
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    types::Shared,
    ui::templates::{GlobalPauseInfo, HomeTemplate},
};

pub async fn home<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
) -> Result<Response, ApiError>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
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

    let template = HomeTemplate {
        global_pause: global_pause_info,
    };
    Ok(Html(
        template
            .render()
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("Template error: {}", e)))?,
    )
    .into_response())
}
