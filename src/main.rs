use std::{env, sync::Arc};

use anyhow::{Context, Result};
use blackholed::{
    db::SqlDb,
    server::{start_server, ServerConfig},
};
use hickory_server::resolver::config::NameServerConfigGroup;

#[tokio::main]
async fn main() -> Result<()> {
    if env::var("BLACKHOLE_LOG").is_err() {
        env::set_var("BLACKHOLE_LOG", "info");
    }
    pretty_env_logger::try_init_timed_custom_env("BLACKHOLE_LOG")?;

    log::info!("Initializing");

    let config = ServerConfig {
        upstream: NameServerConfigGroup::cloudflare_tls(),
        port: 5353,
        cache_size: 10000,
    };

    let db = Arc::new(
        SqlDb::new_sqlite("blackholed.db")
            .await
            .context("Failed to initialize SQLite")?,
    );

    start_server(config, db)
        .await
        .context("Failed to start server")?
        .block_until_done()
        .await
        .context("DNS server exited with an error")
}
