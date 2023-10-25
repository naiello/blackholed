use async_trait::async_trait;
use anyhow::Result;
use futures::TryFutureExt;

use crate::config::BlocklistConfig;

#[async_trait]
pub trait Loader {
    async fn load(&self, blocklist: &BlocklistConfig) -> Result<String>;
}

pub struct WebLoader {
}

impl WebLoader {
    pub fn new() -> Self {
        WebLoader { }
    }
}

#[async_trait]
impl Loader for WebLoader {
    async fn load(&self, blocklist: &BlocklistConfig) -> Result<String> {
        log::info!("downloading blocklist '{}' from: {}", blocklist.name, blocklist.url);

        let raw = reqwest::get(blocklist.url.to_owned())
            .await?
            .text()
            .inspect_err(|e| log::warn!("failed to download blocklist '{}': {}", blocklist.name, e))
            .await?;

        log::info!("download complete for blocklist '{}'", blocklist.name);
        Ok(raw)
    }
}

