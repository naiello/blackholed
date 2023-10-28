use async_trait::async_trait;
use anyhow::Result;
use futures::TryFutureExt;

use crate::config::BlocklistConfig;

#[async_trait]
pub trait Loader {
    async fn load(&self, name: &str, cfg: &BlocklistConfig) -> Result<String>;
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
    async fn load(&self, name: &str, cfg: &BlocklistConfig) -> Result<String> {
        log::info!("downloading blocklist '{}' from: {}", name, cfg.url);

        let raw = reqwest::get(cfg.url.to_owned())
            .await?
            .text()
            .inspect_err(|e| log::warn!("failed to download cfg '{}': {}", name, e))
            .await?;

        log::info!("download complete for cfg '{}'", name);
        Ok(raw)
    }
}

