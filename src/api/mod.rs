pub mod error;
pub mod handlers;
pub mod model;
pub mod state;
pub mod validation;

use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    Router,
    routing::{delete, get, post},
};
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;
use tower_http::{services::ServeDir, trace::TraceLayer};

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    config::ApiConfig,
    db::Db,
    eventstore::EventStore,
    sourceloader::SourceLoader,
    types::Shared,
};
use state::ApiState;

pub struct Api {
    _handle: AbortOnDropHandle<()>,
}

impl Api {
    pub async fn new<DB, BP, ES>(
        config: ApiConfig,
        db: Arc<DB>,
        blocklist: Arc<BlocklistAuthority<BP>>,
        eventstore: Arc<ES>,
        sourceloader: Arc<SourceLoader>,
        shutdown: ShutdownGuard,
    ) -> Result<Self>
    where
        DB: Db + Shared,
        BP: BlocklistProvider + Shared,
        ES: EventStore + Shared,
    {
        let state = ApiState::new(db, blocklist, eventstore, sourceloader);
        let ui_routes = crate::ui::create_router(state.clone());
        let api_routes = create_api_router(state);

        let app = Router::new()
            .merge(ui_routes)
            .merge(api_routes)
            .nest_service("/static", ServeDir::new("static"));

        let addr = format!("0.0.0.0:{}", config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;

        tracing::info!(addr, "Management API and Web UI listening");

        let handle = shutdown.into_spawn_task_fn(|guard| async move {
            let server = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                guard.cancelled().await;
            });

            if let Err(err) = server.await {
                tracing::error!(error = %err, "Server died unexpectedly")
            }
        });

        Ok(Self {
            _handle: AbortOnDropHandle::new(handle),
        })
    }
}

fn create_api_router<DB, BP, ES>(state: ApiState<DB, BP, ES>) -> Router
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
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
        .route(
            "/api/pause/global",
            post(handlers::pauses::start_global_pause),
        )
        .route(
            "/api/pause/global",
            delete(handlers::pauses::stop_global_pause),
        )
        .route("/api/pause/global", get(handlers::pauses::get_global_pause))
        .route(
            "/api/pause/clients/:ip",
            post(handlers::pauses::start_client_pause),
        )
        .route(
            "/api/pause/clients/:ip",
            delete(handlers::pauses::stop_client_pause),
        )
        .route(
            "/api/pause/clients/:ip",
            get(handlers::pauses::get_client_pause),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}
