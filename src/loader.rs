use async_trait::async_trait;
use anyhow::Result;
use futures::TryFutureExt;
use std::str;

use crate::config::DomainListSource;

#[async_trait]
pub trait Loader {
    async fn load(&self, name: &str, source: &DomainListSource) -> Result<String>;
}

pub struct DefaultLoader {
}

impl DefaultLoader {
    pub fn new() -> Self {
        DefaultLoader { }
    }
}

#[async_trait]
impl Loader for DefaultLoader {
    async fn load(&self, name: &str, source: &DomainListSource) -> Result<String> {
        match source {
            DomainListSource::Web { url } => self.load_web(name, url).await,
            DomainListSource::Local { file } => self.load_file(name, file).await,
        }
    }
}

impl DefaultLoader {
    async fn load_web(&self, name: &str, url: &str) -> Result<String> {
        log::info!("downloading list '{}' from: {}", name, url);

        let raw = reqwest::get(url.to_owned())
            .await?
            .error_for_status()?
            .text()
            .inspect_err(|e| log::warn!("failed to download list '{}': {}", name, e))
            .await?;

        log::info!("download complete for list '{}'", name);

        Ok(raw)
    }

    async fn load_file(&self, name: &str, filename: &str) -> Result<String> {
        log::info!("loading list '{}' from file: {}", name, filename);
        let raw = tokio::fs::read(filename).await?;
        let decoded = str::from_utf8(&raw)?;
        Ok(decoded.to_owned())
    }
}
