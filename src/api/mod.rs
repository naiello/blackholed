pub mod error;
pub mod handlers;
pub mod model;
pub mod state;
pub mod validation;

use std::sync::Arc;

use anyhow::Result;
use axum::{
    routing::{delete, get, post},
    Router,
};
use tokio_util::task::AbortOnDropHandle;
use tower_http::trace::TraceLayer;

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    config::ApiConfig,
    db::Db,
    eventstore::EventStore,
    sourceloader::SourceLoaderHandle,
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
        sourceloader: Arc<SourceLoaderHandle>,
    ) -> Result<Self>
    where
        DB: Db + Shared,
        BP: BlocklistProvider + Shared,
        ES: EventStore + Shared,
    {
        let mut task = ApiTask {
            config,
            state: ApiState::new(db, blocklist, eventstore, sourceloader),
        };
        let handle = tokio::spawn(async move {
            task.run().await.ok();
        });

        Ok(Self {
            _handle: AbortOnDropHandle::new(handle),
        })
    }
}

pub struct ApiTask<DB, BP, ES>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    config: ApiConfig,
    state: ApiState<DB, BP, ES>,
}

impl<DB, BP, ES> ApiTask<DB, BP, ES>
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    /// Create the API router with all routes
    fn create_api_router(&self) -> Router {
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
            .with_state(self.state.clone())
            .layer(TraceLayer::new_for_http())
    }

    /// Start the API server on the specified port
    async fn run(&mut self) -> anyhow::Result<()> {
        use std::net::SocketAddr;
        use tower_http::services::ServeDir;

        let api_routes = self.create_api_router();
        let ui_routes = crate::ui::create_router(self.state.clone());

        let app = Router::new()
            .merge(ui_routes)
            .merge(api_routes)
            .nest_service("/static", ServeDir::new("static"));

        let addr = format!("0.0.0.0:{}", self.config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;

        log::info!("Management API and Web UI listening on {}", addr);

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;

        Ok(())
    }
}
