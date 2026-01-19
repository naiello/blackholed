use std::{env, sync::Arc};

use anyhow::{Context, Result};
use blackholed::{
    api::{self, state::ApiState},
    blocklist::BlocklistAuthority,
    db::{Db, SqlDb},
    eventstore::RedisEventStore,
    model::{HostDisposition, Source},
    resolver,
    sourceloader::{SourceLoader, SourceLoaderConfig},
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

    let eventstore = Arc::new(
        RedisEventStore::new(Default::default())
            .await
            .context("Failed to initialize Redis EventStore")?,
    );

    let db = Arc::new(
        SqlDb::new_sqlite("blackholed.db")
            .await
            .context("Failed to initialize SQLite")?,
    );

    let blocklist = Arc::new(
        BlocklistAuthority::new(
            Name::root(),
            &Default::default(),
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

    let _loader = SourceLoader::new(SourceLoaderConfig::default(), db.clone(), blocklist.clone())
        .await
        .context("Failed to start SourceLoader")?;

    // Create API state and spawn server
    let api_state = ApiState::new(db.clone(), blocklist.clone(), eventstore.clone());

    let api_handle = tokio::spawn(async move {
        if let Err(e) = api::start_server(5355, api_state).await {
            log::error!("API server error: {}", e);
        }
    });

    // Start DNS resolver
    let mut dns_server = resolver::start(Default::default(), blocklist.clone())
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
