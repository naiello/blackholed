use std::{env, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use blackholed::{
    api::Api,
    blocklist::BlocklistAuthority,
    config::Config,
    db::{Db, SqlDb},
    eventstore::RedisEventStore,
    model::{HostDisposition, Source},
    resolver::Resolver,
    sourceloader::SourceLoader,
};
use chrono::Utc;
use hickory_server::resolver::Name;
use tokio_graceful::Shutdown;

#[tokio::main]
async fn main() -> Result<()> {
    if env::var("BLACKHOLE_LOG").is_err() {
        unsafe {
            env::set_var(
                "BLACKHOLE_LOG",
                "info,hickory_server::server=warn,tokio_graceful::shutdown=warn",
            );
        }
    }
    pretty_env_logger::try_init_timed_custom_env("BLACKHOLE_LOG")?;

    log::info!("Initializing");

    let config = Config::load().context("Failed to load configuration")?;
    log::info!("Configuration loaded successfully");

    let shutdown = Shutdown::default();

    let eventstore = Arc::new(
        RedisEventStore::new(config.eventstore, shutdown.guard())
            .await
            .context("Failed to initialize Redis EventStore")?,
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
            shutdown.guard(),
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

    let sourceloader = Arc::new(
        SourceLoader::new(
            config.sourceloader.run_interval(),
            config.sourceloader.stale_age(),
            db.clone(),
            blocklist.clone(),
            config.sourceloader.blocklist_dirs,
            config.sourceloader.allowlist_dirs,
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

    log::info!("Shutdown complete");

    Ok(())
}
