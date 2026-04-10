use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tracing_subscriber::{EnvFilter, fmt};
use blackholed::{
    api::Api,
    blocklist::BlocklistAuthority,
    config::Config,
    db::{Db, SqlDb},
    eventstore::RedisEventStore,
    model::{HostDisposition, Source},
    notifier::BlocklistNotifier,
    resolver::Resolver,
    sourceloader::SourceLoader,
};
use chrono::Utc;
use hickory_server::resolver::Name;
use tokio_graceful::Shutdown;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("BLACKHOLE_LOG").unwrap_or_else(|_| {
        EnvFilter::new("info,hickory_server::server=warn,tokio_graceful::shutdown=warn")
    });
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        fmt().with_env_filter(filter).init();
    } else {
        fmt().json().with_env_filter(filter).init();
    }

    tracing::info!("Initializing");

    let config = Config::load().context("Failed to load configuration")?;
    tracing::info!("Configuration loaded successfully");

    let shutdown = Shutdown::default();

    let redis_url = config.eventstore.endpoint.clone();
    let eventstore = Arc::new(
        RedisEventStore::new(config.eventstore, shutdown.guard())
            .await
            .context("Failed to initialize Redis EventStore")?,
    );

    let notifier = Arc::new(
        BlocklistNotifier::new(&redis_url)
            .await
            .context("Failed to initialize blocklist notifier")?,
    );

    let db = Arc::new(
        SqlDb::new(&config.database)
            .await
            .context("Failed to initialize database")?,
    );

    let blocklist = Arc::new(
        BlocklistAuthority::new(
            Name::root(),
            &config.blocklist,
            db.clone(),
            eventstore.clone(),
            notifier.clone(),
            shutdown.guard(),
        )
        .await,
    );

    let _subscriber = notifier.spawn_subscriber(blocklist.clone(), shutdown.guard());

    if db.get_source("webmanaged").await.is_err() {
        tracing::info!("Creating webmanaged allowlist source");
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

    let sourceloader = Arc::new(
        SourceLoader::new(
            config.sourceloader.run_interval(),
            config.sourceloader.stale_age(),
            db.clone(),
            blocklist.clone(),
            config.sourceloader.blocklist_dirs,
            config.sourceloader.allowlist_dirs,
            redis_url,
            shutdown.guard(),
        )
        .await
        .context("Failed to start SourceLoader")?,
    );

    let _api = Api::new(
        config.api,
        db.clone(),
        blocklist.clone(),
        eventstore.clone(),
        sourceloader,
        shutdown.guard(),
    )
    .await
    .context("Failed to start API server")?;

    let _resolver = Resolver::new(config.resolver, blocklist.clone(), shutdown.guard())
        .await
        .context("Failed to start resolver")?;

    shutdown
        .shutdown_with_limit(Duration::from_mins(1))
        .await
        .context("Error while performing graceful shutdown")?;

    tracing::info!("Shutdown complete");

    Ok(())
}
