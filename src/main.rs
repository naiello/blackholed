use futures::TryFutureExt;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use anyhow::Result;
use simple_logger::SimpleLogger;

use blackholed::config::*;

async fn download(name: &str, url: &str) -> Result<()> {
    let content = reqwest::get(url).await?.bytes().await?;
    let filename = format!("./blocklists/{}.hosts", name);

    tokio::fs::write(filename, content).await?;

    log::info!("download complete: {}", url);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    SimpleLogger::new().init().unwrap();

    let config = Config::load();

    config.blocklists
        .iter()
        .map(|BlocklistConfig { name, url }| download(name, url)
             .inspect_err(move |e| log::warn!("download of {} failed: {}", url, e)))
        .collect::<FuturesUnordered<_>>()
        .collect::<Vec<_>>()
        .await;

    Ok(())
}
