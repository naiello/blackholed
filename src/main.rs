use std::{env, sync::Arc};

use anyhow::{Context, Result};
use blackholed::{
    blocklist::BlocklistAuthority,
    db::SqlDb,
    eventstore::RedisEventStore,
    resolver,
    sourceloader::{SourceLoader, SourceLoaderConfig},
};
use hickory_server::resolver::Name;

#[tokio::main]
async fn main() -> Result<()> {
    if env::var("BLACKHOLE_LOG").is_err() {
        env::set_var("BLACKHOLE_LOG", "info");
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

    let _loader = SourceLoader::new(SourceLoaderConfig::default(), db, blocklist.clone())
        .await
        .context("Failed to start SourceLoader")?;

    resolver::start(Default::default(), blocklist.clone())
        .await
        .context("Failed to start server")?
        .block_until_done()
        .await
        .context("DNS server exited with an error")
}
