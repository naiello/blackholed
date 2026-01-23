use std::{env, sync::Arc};

use anyhow::{Context, Result};
use blackholed::{
    api::{self, state::ApiState},
    blocklist::BlocklistAuthority,
    config::Config,
    db::{Db, SqlDb},
    eventstore::RedisEventStore,
    model::{HostDisposition, Source},
    resolver,
    sourceloader::SourceLoader,
};
use chrono::Utc;
use hickory_server::resolver::Name;

#[tokio::main]
async fn main() -> Result<()> {
    if env::var("BLACKHOLE_LOG").is_err() {
        env::set_var("BLACKHOLE_LOG", "info,hickory_server::server=warn");
    }
    pretty_env_logger::try_init_timed_custom_env("BLACKHOLE_LOG")?;

    log::info!("Initializing");

    let config = Config::load().context("Failed to load configuration")?;
    log::info!("Configuration loaded successfully");

    let eventstore = Arc::new(
        RedisEventStore::new(
            config.eventstore.endpoint.clone(),
            config.eventstore.event_ttl(),
            config.eventstore.client_ttl(),
            config.eventstore.sweeper_interval(),
        )
        .await
        .context("Failed to initialize Redis EventStore")?,
    );

    let db = Arc::new(
        SqlDb::new_sqlite(&config.database.path)
            .await
            .context("Failed to initialize SQLite")?,
    );

    let blocklist = Arc::new(
        BlocklistAuthority::new(
            Name::root(),
            &config.blocklist,
            db.clone(),
            eventstore.clone(),
        )
        .await,
    );

    if db.get_source("webmanaged").await.is_err() {
        log::info!("Creating webmanaged allowlist source");
        let source = Source {
            id: "webmanaged".to_string(),
            url: None,
            path: None,
            disposition: HostDisposition::Allow,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        db.put_source(source)
            .await
            .context("Failed to create webmanaged source")?;
    }

    let (_loader, loader_handle) = SourceLoader::new(
        config.sourceloader.run_interval(),
        config.sourceloader.stale_age(),
        db.clone(),
        blocklist.clone(),
    )
    .await
    .context("Failed to start SourceLoader")?;

    // Create API state and spawn server
    let api_state = ApiState::new(
        db.clone(),
        blocklist.clone(),
        eventstore.clone(),
        Arc::new(loader_handle),
    );

    let api_port = config.api.port;
    let api_handle = tokio::spawn(async move {
        if let Err(e) = api::start_server(api_port, api_state).await {
            log::error!("API server error: {}", e);
        }
    });

    // Start DNS resolver
    let mut dns_server = resolver::start(
        config.resolver.port,
        config.resolver.upstream.to_nameserver_config_group()?,
        config.resolver.cache_size,
        config.resolver.zones,
        blocklist.clone(),
    )
    .await
    .context("Failed to start server")?;

    // Wait for both servers
    tokio::select! {
        result = dns_server.block_until_done() => {
            result.context("DNS server exited with an error")
        }
        _ = api_handle => {
            anyhow::bail!("API server exited unexpectedly")
        }
    }
}
