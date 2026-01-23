use anyhow;
use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use chrono::Utc;

use crate::{
    api::{error::ApiError, state::ApiState},
    db::SqlDb,
    eventstore::{EventStore, RedisEventStore},
    ui::templates::{GlobalPauseInfo, HomeTemplate},
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

pub async fn home(State(state): State<ConcreteState>) -> Result<Response, ApiError> {
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
