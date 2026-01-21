pub mod handlers;
pub mod templates;

use axum::{
    routing::{delete, get, post},
    Router,
};

use crate::{
    api::state::ApiState, db::SqlDb, eventstore::RedisEventStore,
};
use sqlx::Sqlite;

type ConcreteState = ApiState<SqlDb<Sqlite>, SqlDb<Sqlite>, RedisEventStore>;

pub fn create_router(state: ConcreteState) -> Router {
    Router::new()
        .route("/", get(handlers::home::home))
        .route("/events", get(handlers::events::events_default))
        .route("/events/:ip", get(handlers::events::events_by_ip))
        .route("/allowlist", post(handlers::events::allowlist_domain))
        .route("/clients", get(handlers::clients::list_clients))
        .route("/clients/:ip", get(handlers::clients::client_detail))
        .route("/sources", get(handlers::sources::list_sources))
        .route("/sources/new", get(handlers::sources::new_source_form))
        .route("/sources", post(handlers::sources::create_source))
        .route("/sources/:id", get(handlers::sources::source_detail))
        .route(
            "/sources/:id/hosts",
            post(handlers::sources::add_host),
        )
        .route(
            "/sources/:id/hosts/:name",
            delete(handlers::sources::delete_host),
        )
        .route(
            "/sources/:id/delete",
            post(handlers::sources::delete_source),
        )
        .with_state(state)
}
