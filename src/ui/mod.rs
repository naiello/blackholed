pub mod handlers;
pub mod templates;

use axum::{
    Router,
    routing::{delete, get, post},
};

use crate::{
    api::state::ApiState, blocklist::BlocklistProvider, db::Db, eventstore::EventStore,
    types::Shared,
};

pub fn create_router<DB, BP, ES>(state: ApiState<DB, BP, ES>) -> Router
where
    DB: Db + Shared,
    BP: BlocklistProvider + Shared,
    ES: EventStore + Shared,
{
    Router::new()
        .route("/", get(handlers::home::home))
        .route("/allowlist", post(handlers::clients::allowlist_domain))
        .route("/clients", get(handlers::clients::list_clients))
        .route("/clients/:ip", get(handlers::clients::client_detail))
        .route("/sources", get(handlers::sources::list_sources))
        .route("/sources/new", get(handlers::sources::new_source_form))
        .route("/sources", post(handlers::sources::create_source))
        .route("/sources/:id", get(handlers::sources::source_detail))
        .route(
            "/sources/:id/reload",
            post(handlers::sources::reload_source),
        )
        .route("/sources/:id/hosts", post(handlers::sources::add_host))
        .route(
            "/sources/:id/hosts/:name",
            delete(handlers::sources::delete_host),
        )
        .route(
            "/sources/:id/delete",
            post(handlers::sources::delete_source),
        )
        .route("/pause/global", post(handlers::pauses::start_global_pause))
        .route("/pause/global", delete(handlers::pauses::stop_global_pause))
        .route(
            "/pause/clients/:ip",
            post(handlers::pauses::start_client_pause),
        )
        .route(
            "/pause/clients/:ip",
            delete(handlers::pauses::stop_client_pause),
        )
        .with_state(state)
}
