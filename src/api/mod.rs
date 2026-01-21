pub mod error;
pub mod handlers;
pub mod model;
pub mod state;
pub mod validation;

use axum::{
    routing::{delete, get, post},
    Router,
};
use tower_http::trace::TraceLayer;

use crate::{db::SqlDb, eventstore::RedisEventStore};
use sqlx::Sqlite;
use state::ApiState;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

/// Create the API router with all routes
pub fn create_api_router(state: ConcreteState) -> Router {
    Router::new()
        .route("/api/sources", post(handlers::sources::create_source))
        .route("/api/sources", get(handlers::sources::list_sources))
        .route("/api/sources/:id", get(handlers::sources::get_source))
        .route("/api/sources/:id", delete(handlers::sources::delete_source))
        .route(
            "/api/sources/:id/reload",
            post(handlers::sources::reload_source),
        )
        .route(
            "/api/sources/:source_id/hosts",
            post(handlers::hosts::create_host),
        )
        .route(
            "/api/sources/:source_id/hosts",
            get(handlers::hosts::list_hosts),
        )
        .route(
            "/api/sources/:source_id/hosts/:name",
            delete(handlers::hosts::delete_host),
        )
        .route("/api/clients", get(handlers::events::list_clients))
        .route(
            "/api/clients/:ip/events",
            get(handlers::events::list_client_events),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Start the API server on the specified port
pub async fn start_server(port: u16, state: ConcreteState) -> anyhow::Result<()> {
    use std::net::SocketAddr;
    use tower_http::services::ServeDir;

    let api_routes = create_api_router(state.clone());
    let ui_routes = crate::ui::create_router(state);

    let app = Router::new()
        .merge(ui_routes)
        .merge(api_routes)
        .nest_service("/static", ServeDir::new("static"));

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    log::info!("Management API and Web UI listening on {}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
