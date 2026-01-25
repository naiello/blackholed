use axum::{
    extract::{Path, State},
    response::Html,
    Form,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use std::net::IpAddr;
use std::str::FromStr;

use crate::{
    api::{
        error::{ApiError, ApiResult},
        state::ApiState,
    },
    blocklist::BlocklistProvider,
    db::Db,
    eventstore::EventStore,
    types::Shared,
};

#[derive(Deserialize)]
pub struct StartPauseForm {
    pub duration_seconds: String,
}

/// POST /pause/global - Start a global pause (UI handler)
pub async fn start_global_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Form(form): Form<StartPauseForm>,
) -> ApiResult<Html<String>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    let duration_seconds: i64 = form
        .duration_seconds
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid duration".into()))?;

    // Calculate expiration time
    let expires_at = if duration_seconds >= 31536000000 {
        // Permanent (>1000 years) - use year 9999
        DateTime::parse_from_rfc3339("9999-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    } else {
        Utc::now() + Duration::seconds(duration_seconds)
    };

    // Write to EventStore for persistence
    state
        .eventstore
        .put_global_pause(expires_at)
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_global_pause(true);

    // Return the active pause banner HTML
    let html = format!(
        "<div id=\"global-pause-banner\" class=\"mb-6 p-4 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded-lg\"> \
    <div class=\"flex items-center justify-between\"> \
        <div> \
            <h3 class=\"font-semibold text-blue-900 dark:text-blue-100\">Global Pause Active</h3> \
            <p class=\"text-sm text-blue-800 dark:text-blue-200\"> \
                Blocking will resume \
                <time class=\"local-time\" datetime=\"{}\"> \
                    {} \
                </time> \
            </p> \
        </div> \
        <button \
            hx-delete=\"/pause/global\" \
            hx-target=\"#global-pause-banner\" \
            hx-swap=\"outerHTML\" \
            class=\"bg-blue-600 dark:bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-700 dark:hover:bg-blue-600 transition\"> \
            End Pause \
        </button> \
    </div> \
</div>",
        expires_at.to_rfc3339(),
        expires_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    Ok(Html(html))
}

/// DELETE /pause/global - Stop the global pause (UI handler)
pub async fn stop_global_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
) -> ApiResult<Html<String>>
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

    // Return the pause control HTML
    let html = "<div id=\"global-pause-banner\" class=\"mb-6 p-4 bg-gray-50 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg\"> \
    <h3 class=\"font-semibold text-gray-900 dark:text-gray-100 mb-3\">Pause Blocking Globally</h3> \
    <form hx-post=\"/pause/global\" hx-swap=\"outerHTML\" hx-target=\"#global-pause-banner\" class=\"flex gap-2\"> \
        <select name=\"duration_seconds\" class=\"flex-1 border border-gray-300 dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500\"> \
            <option value=\"3600\">1 hour</option> \
            <option value=\"86400\">1 day</option> \
            <option value=\"604800\">7 days</option> \
            <option value=\"2592000\">30 days</option> \
            <option value=\"252423993600\">Permanent</option> \
        </select> \
        <button type=\"submit\" class=\"bg-yellow-600 dark:bg-yellow-500 text-white px-4 py-2 rounded hover:bg-yellow-700 dark:hover:bg-yellow-600 transition\"> \
            Pause \
        </button> \
    </form> \
</div>";

    Ok(Html(html.to_string()))
}

/// POST /pause/clients/:ip - Start a client pause (UI handler)
pub async fn start_client_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(ip): Path<String>,
    Form(form): Form<StartPauseForm>,
) -> ApiResult<Html<String>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    let ip_addr = IpAddr::from_str(&ip)
        .map_err(|_| ApiError::BadRequest(format!("Invalid IP address: {}", ip)))?;

    let duration_seconds: i64 = form
        .duration_seconds
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid duration".into()))?;

    // Calculate expiration time
    let expires_at = if duration_seconds >= 31536000000 {
        // Permanent (>1000 years) - use year 9999
        DateTime::parse_from_rfc3339("9999-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    } else {
        Utc::now() + Duration::seconds(duration_seconds)
    };

    // Write to EventStore for persistence
    state
        .eventstore
        .put_client_pause(ip_addr, expires_at)
        .await
        .map_err(ApiError::Internal)?;

    // Update BlocklistAuthority for immediate effect
    state.blocklist.set_client_pause(ip_addr, true).await;

    // Return the active pause banner HTML
    let safe_ip = ip.replace(['.', ':'], "-");
    let html = format!(
        "<div id=\"client-pause-banner-{}\" class=\"mb-6 p-4 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded-lg\"> \
    <div class=\"flex items-center justify-between\"> \
        <div> \
            <h3 class=\"font-semibold text-blue-900 dark:text-blue-100\">Client Pause Active</h3> \
            <p class=\"text-sm text-blue-800 dark:text-blue-200\"> \
                Blocking will resume for <span class=\"font-mono\">{}</span> \
                <time class=\"local-time\" datetime=\"{}\"> \
                    {} \
                </time> \
            </p> \
        </div> \
        <button \
            hx-delete=\"/pause/clients/{}\" \
            hx-target=\"#client-pause-banner-{}\" \
            hx-swap=\"outerHTML\" \
            class=\"bg-blue-600 dark:bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-700 dark:hover:bg-blue-600 transition\"> \
            End Pause \
        </button> \
    </div> \
</div>",
        safe_ip,
        ip,
        expires_at.to_rfc3339(),
        expires_at.format("%Y-%m-%d %H:%M:%S UTC"),
        ip,
        safe_ip
    );

    Ok(Html(html))
}

/// DELETE /pause/clients/:ip - Stop the client pause (UI handler)
pub async fn stop_client_pause<DB, BP, ES>(
    State(state): State<ApiState<DB, BP, ES>>,
    Path(ip): Path<String>,
) -> ApiResult<Html<String>>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
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

    // Return the pause control HTML
    let safe_ip = ip.replace(['.', ':'], "-");
    let html = format!(
        "<div id=\"client-pause-banner-{}\" class=\"mb-6 p-4 bg-gray-50 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg\"> \
    <h3 class=\"font-semibold text-gray-900 dark:text-gray-100 mb-3\">Pause Blocking for <span class=\"font-mono\">{}</span></h3> \
    <form hx-post=\"/pause/clients/{}\" hx-swap=\"outerHTML\" hx-target=\"#client-pause-banner-{}\" class=\"flex gap-2\"> \
        <select name=\"duration_seconds\" class=\"flex-1 border border-gray-300 dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500\"> \
            <option value=\"3600\">1 hour</option> \
            <option value=\"86400\">1 day</option> \
            <option value=\"604800\">7 days</option> \
            <option value=\"2592000\">30 days</option> \
            <option value=\"252423993600\">Permanent</option> \
        </select> \
        <button type=\"submit\" class=\"bg-yellow-600 dark:bg-yellow-500 text-white px-4 py-2 rounded hover:bg-yellow-700 dark:hover:bg-yellow-600 transition\"> \
            Pause \
        </button> \
    </form> \
</div>",
        safe_ip, ip, ip, safe_ip
    );

    Ok(Html(html))
}
