use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use futures::StreamExt;
use tokio::{
    io::{AsyncBufRead, BufReader},
    sync::mpsc,
    time,
};
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    db::Db,
    model::{Source, SourceHost},
    types::Shared,
};

pub struct SourceLoader {
    _task: AbortOnDropHandle<()>,
    reload_tx: mpsc::Sender<String>,
}

struct SourceLoaderTask<DB: Db, BP: BlocklistProvider> {
    db: Arc<DB>,
    blocklist_authority: Arc<BlocklistAuthority<BP>>,
    run_interval: Duration,
    stale_age: TimeDelta,
    reload_rx: mpsc::Receiver<String>,
}

impl SourceLoader {
    pub async fn new<DB, BP>(
        run_interval: TimeDelta,
        stale_age: TimeDelta,
        db: Arc<DB>,
        blocklist_authority: Arc<BlocklistAuthority<BP>>,
        shutdown: ShutdownGuard,
    ) -> Result<Self>
    where
        DB: Db + Shared,
        BP: BlocklistProvider + Shared,
    {
        let (reload_tx, reload_rx) = mpsc::channel(32);

        let task = SourceLoaderTask {
            db,
            blocklist_authority,
            stale_age,
            run_interval: run_interval.to_std().context("Invalid run interval")?,
            reload_rx,
        };
        let handle = shutdown.spawn_task_fn(|guard| async move { task.run(guard).await });
        Ok(SourceLoader {
            _task: AbortOnDropHandle::new(handle),
            reload_tx,
        })
    }

    pub async fn reload_source(&self, source_id: String) -> Result<()> {
        self.reload_tx
            .send(source_id)
            .await
            .context("Failed to send reload request")
    }
}

impl<DB: Db, BP: BlocklistProvider> SourceLoaderTask<DB, BP> {
    async fn run(mut self, shutdown: ShutdownGuard) {
        log::info!("Starting SourceLoader background task");

        // Perform initial refresh immediately on startup
        log::info!("Performing initial source refresh");
        if let Err(err) = self.refresh_all().await {
            log::error!("Error during initial source refresh: {:?}", err);
        }

        // Then enter interval loop
        let mut interval = time::interval(self.run_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.refresh_all().await {
                        log::error!("Error while refreshing sources: {:?}", err);
                    }
                }
                Some(source_id) = self.reload_rx.recv() => {
                    log::info!("Manual reload requested for source: {}", source_id);
                    if let Err(err) = self.handle_manual_reload(source_id).await {
                        log::error!("Error during manual reload: {:?}", err);
                    }
                }
                _ = shutdown.cancelled() => {
                    log::info!("Source loader shutting down");
                    break;
                }
            }
        }
    }

    async fn handle_manual_reload(&self, source_id: String) -> Result<()> {
        log::info!("Processing manual reload for source: {}", source_id);

        // Fetch source from database
        let source = match self.db.get_source(&source_id).await {
            Ok(source) => source,
            Err(err) => {
                log::warn!("Source {} not found: {:?}", source_id, err);
                bail!("Source not found");
            }
        };

        // Verify source is auto-managed (has URL or path)
        if source.url.is_none() && source.path.is_none() {
            log::warn!(
                "Source {} is manually-managed, cannot reload from URL/path",
                source_id
            );
            bail!("Source is manually-managed");
        }

        // Perform refresh
        let refresh_time = Utc::now();
        match self.refresh(source.clone(), refresh_time).await {
            Ok(_) => {
                log::info!(
                    "Successfully completed manual reload for source: {}",
                    source_id
                );
                self.blocklist_authority.reload().await;
                Ok(())
            }
            Err(err) => {
                log::error!("Failed manual reload for source {}: {:?}", source_id, err);
                Err(err)
            }
        }
    }

    async fn refresh_all(&self) -> Result<()> {
        log::info!("Checking for stale sources");

        let start_time = Utc::now();
        let stale_threshold = start_time - self.stale_age;
        let all_sources: Vec<Source> = self.db.get_all_sources().collect().await;
        let sources_to_refresh: Vec<Source> = all_sources
            .into_iter()
            .filter(|source| {
                // Only refresh sources with URL or path defined
                (source.url.is_some() || source.path.is_some())
                    // Only refresh stale sources
                    && source.updated_at < stale_threshold
            })
            .collect();

        if sources_to_refresh.is_empty() {
            return Ok(());
        }

        log::info!(
            "Found {} stale sources to refresh",
            sources_to_refresh.len()
        );

        let mut successful_refreshes = 0;
        let mut failed_refreshes = 0;

        for source in sources_to_refresh {
            match self.refresh(source.clone(), start_time).await {
                Ok(_) => {
                    successful_refreshes += 1;
                    log::info!("Successfully refreshed source: {}", source.id);
                }
                Err(err) => {
                    failed_refreshes += 1;
                    log::error!("Failed to refresh source {}: {:?}", source.id, err);
                }
            }
        }

        log::info!(
            "Source refresh cycle complete: {} successful, {} failed",
            successful_refreshes,
            failed_refreshes
        );

        if successful_refreshes > 0 {
            self.blocklist_authority.reload().await;
        }

        Ok(())
    }

    async fn refresh(&self, source: Source, refresh_time: DateTime<Utc>) -> Result<()> {
        log::debug!("Refreshing source: {}", source.id);

        let content = self
            .fetch_content(&source)
            .await
            .context("Failed to fetch source content")?;

        let parsed_hosts = crate::parser::parse_list(content)
            .await
            .context("Failed to parse source content")?;

        log::info!(
            "Parsed {} hosts from source {}",
            parsed_hosts.len(),
            source.id
        );

        let hosts: Vec<SourceHost> = parsed_hosts
            .into_iter()
            .map(|parsed_host| SourceHost {
                name: parsed_host.name.to_string(),
                source_id: source.id.clone(),
                disposition: source.disposition,
                created_at: refresh_time,
                updated_at: refresh_time,
            })
            .collect();

        self.db
            .put_hosts(hosts)
            .await
            .context("Failed to upsert hosts")?;

        let stale_hosts: Vec<SourceHost> = self
            .db
            .get_stale_hosts_by_source(&source.id, refresh_time)
            .collect()
            .await;

        log::debug!(
            "Deleting {} stale hosts from source {}",
            stale_hosts.len(),
            source.id
        );

        for stale_host in stale_hosts {
            self.db
                .delete_host(&stale_host.name, &source.id)
                .await
                .with_context(|| format!("Failed to delete stale host: {}", stale_host.name))?;
        }

        let updated_source = Source {
            updated_at: refresh_time,
            ..source
        };

        self.db
            .put_source(updated_source)
            .await
            .context("Failed to update source timestamp")?;

        Ok(())
    }

    async fn fetch_content(
        &self,
        source: &Source,
    ) -> Result<Pin<Box<dyn AsyncBufRead + Send + Unpin>>> {
        if let Some(url) = &source.url {
            log::debug!("Fetching source from URL: {}", url);

            let response = reqwest::get(url)
                .await
                .with_context(|| format!("Failed to download from URL: {}", url))?;

            if !response.status().is_success() {
                bail!("HTTP request failed with status: {}", response.status());
            }

            let bytes = response
                .bytes()
                .await
                .context("Failed to read response body")?;

            Ok(Box::pin(BufReader::new(std::io::Cursor::new(
                bytes.to_vec(),
            ))))
        } else if let Some(path) = &source.path {
            log::debug!("Loading source from file: {}", path);

            let file = tokio::fs::File::open(path)
                .await
                .with_context(|| format!("Failed to open file: {}", path))?;

            Ok(Box::pin(BufReader::new(file)))
        } else {
            bail!("Source has neither URL nor path defined");
        }
    }
}
