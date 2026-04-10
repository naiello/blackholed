use std::{collections::HashMap, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeDelta, Utc};
use futures::StreamExt;
use notify::{EventKind, RecommendedWatcher, Watcher};
use tokio::{
    io::{AsyncBufRead, BufReader},
    sync::mpsc,
    time,
};
use tokio_graceful::ShutdownGuard;
use tokio_util::{io::StreamReader, task::AbortOnDropHandle};

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    db::Db,
    model::{HostDisposition, Source, SourceHost},
    types::Shared,
};

const RELOAD_LOCK_TTL: Duration = Duration::from_secs(600); // 10 minutes

pub struct SourceLoader {
    _task: AbortOnDropHandle<()>,
    _watcher_task: Option<AbortOnDropHandle<()>>,
    reload_tx: mpsc::Sender<String>,
}

struct SourceLoaderTask<DB: Db, BP: BlocklistProvider> {
    db: Arc<DB>,
    blocklist_authority: Arc<BlocklistAuthority<BP>>,
    lock_manager: rslock::LockManager,
    http_client: reqwest::Client,
    run_interval: Duration,
    stale_age: TimeDelta,
    reload_rx: mpsc::Receiver<String>,
    file_change_rx: mpsc::Receiver<()>,
    blocklist_dirs: Vec<PathBuf>,
    allowlist_dirs: Vec<PathBuf>,
}

/// A file discovered in a watched directory, paired with its disposition.
struct DiscoveredFile {
    path: PathBuf,
    disposition: HostDisposition,
}

impl SourceLoader {
    pub async fn new<DB, BP>(
        run_interval: TimeDelta,
        stale_age: TimeDelta,
        db: Arc<DB>,
        blocklist_authority: Arc<BlocklistAuthority<BP>>,
        blocklist_dirs: Vec<PathBuf>,
        allowlist_dirs: Vec<PathBuf>,
        redis_url: String,
        shutdown: ShutdownGuard,
    ) -> Result<Self>
    where
        DB: Db + Shared,
        BP: BlocklistProvider + Shared,
    {
        let (reload_tx, reload_rx) = mpsc::channel(32);
        let (file_change_tx, file_change_rx) = mpsc::channel(32);

        // Set up file watcher
        let watcher_task = setup_file_watcher(
            &blocklist_dirs,
            &allowlist_dirs,
            file_change_tx,
            shutdown.clone(),
        );

        let lock_manager = rslock::LockManager::new(vec![redis_url]);

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")?;

        let task = SourceLoaderTask {
            db,
            blocklist_authority,
            lock_manager,
            http_client,
            stale_age,
            run_interval: run_interval.to_std().context("Invalid run interval")?,
            reload_rx,
            file_change_rx,
            blocklist_dirs,
            allowlist_dirs,
        };
        let handle = shutdown.spawn_task_fn(|guard| async move { task.run(guard).await });
        Ok(SourceLoader {
            _task: AbortOnDropHandle::new(handle),
            _watcher_task: watcher_task,
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

/// Derive a source ID from a file path and disposition.
///
/// For `/etc/blackhole/blocklist.d/ads.txt` with Block disposition: `file-block-ads-txt`
/// For `/etc/blackhole/allowlist.d/personal.list` with Allow: `file-allow-personal-list`
fn derive_source_id(path: &std::path::Path, disposition: HostDisposition) -> String {
    let infix = match disposition {
        HostDisposition::Block => "block",
        HostDisposition::Allow => "allow",
    };

    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let sanitized: String = filename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse consecutive hyphens
    let mut collapsed = String::with_capacity(sanitized.len());
    let mut prev_hyphen = false;
    for c in sanitized.chars() {
        if c == '-' {
            if !prev_hyphen {
                collapsed.push(c);
            }
            prev_hyphen = true;
        } else {
            collapsed.push(c);
            prev_hyphen = false;
        }
    }

    // Trim leading/trailing hyphens
    let collapsed = collapsed.trim_matches('-');

    let id = format!("file-{}-{}", infix, collapsed);

    // Truncate to 64 chars
    if id.len() > 64 {
        id[..64].to_string()
    } else {
        id
    }
}

/// Scan configured directories for list files.
fn scan_directories(blocklist_dirs: &[PathBuf], allowlist_dirs: &[PathBuf]) -> Vec<DiscoveredFile> {
    let mut files = Vec::new();

    for (dirs, disposition) in [
        (blocklist_dirs, HostDisposition::Block),
        (allowlist_dirs, HostDisposition::Allow),
    ] {
        for dir in dirs {
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();

                        // Skip directories and symlinks
                        if !path.is_file() {
                            continue;
                        }

                        // Skip dotfiles
                        if let Some(name) = path.file_name().and_then(|n| n.to_str())
                            && name.starts_with('.') {
                                continue;
                            }

                        files.push(DiscoveredFile { path, disposition });
                    }
                }
                Err(err) => {
                    tracing::warn!(dir = %dir.display(), error = %err, "Cannot read directory");
                }
            }
        }
    }

    files
}

/// Set up a filesystem watcher on the configured directories.
/// Returns a task handle that keeps the watcher alive, or None if no directories exist.
fn setup_file_watcher(
    blocklist_dirs: &[PathBuf],
    allowlist_dirs: &[PathBuf],
    change_tx: mpsc::Sender<()>,
    shutdown: ShutdownGuard,
) -> Option<AbortOnDropHandle<()>> {
    let all_dirs: Vec<PathBuf> = blocklist_dirs
        .iter()
        .chain(allowlist_dirs.iter())
        .filter(|d| d.is_dir())
        .cloned()
        .collect();

    if all_dirs.is_empty() {
        tracing::info!("No existing source directories to watch");
        return None;
    }

    // Channel for raw notify events
    let (raw_tx, mut raw_rx) = mpsc::channel::<()>(64);

    let mut watcher = match RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        let _ = raw_tx.try_send(());
                    }
                    _ => {}
                }
            }
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(err) => {
            tracing::error!(error = %err, "Failed to create file watcher");
            return None;
        }
    };

    for dir in &all_dirs {
        if let Err(err) = watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
            tracing::error!(dir = %dir.display(), error = %err, "Failed to watch directory");
        } else {
            tracing::info!(dir = %dir.display(), "Watching directory for changes");
        }
    }

    // Debounce task: drain events over a 2-second window then forward a single signal
    let handle = shutdown.spawn_task_fn(move |guard| async move {
        // Keep watcher alive by moving it into this task
        let _watcher = watcher;

        loop {
            tokio::select! {
                Some(()) = raw_rx.recv() => {
                    // Drain any additional events that arrive within the debounce window
                    time::sleep(Duration::from_secs(2)).await;
                    while raw_rx.try_recv().is_ok() {}
                    let _ = change_tx.send(()).await;
                }
                _ = guard.cancelled() => {
                    tracing::info!("File watcher shutting down");
                    break;
                }
            }
        }
    });

    Some(AbortOnDropHandle::new(handle))
}

impl<DB: Db, BP: BlocklistProvider> SourceLoaderTask<DB, BP> {
    async fn run(mut self, shutdown: ShutdownGuard) {
        tracing::info!("Starting SourceLoader background task");

        // Perform initial file source sync
        if let Err(err) = self.sync_and_refresh_file_sources(true).await {
            tracing::error!(error = ?err, "Error during initial file source sync");
        }

        // Perform initial refresh of URL sources
        tracing::info!("Performing initial source refresh");
        if let Err(err) = self.refresh_all().await {
            tracing::error!(error = ?err, "Error during initial source refresh");
        }

        // Then enter interval loop — tick() fires immediately, so consume the first tick
        // to avoid a duplicate refresh right after the explicit startup call above.
        let mut interval = time::interval(self.run_interval);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.refresh_all().await {
                        tracing::error!(error = ?err, "Error while refreshing sources");
                    }
                }
                Some(source_id) = self.reload_rx.recv() => {
                    tracing::info!(source_id, "Manual reload requested");
                    if let Err(err) = self.handle_manual_reload(source_id).await {
                        tracing::error!(error = ?err, "Error during manual reload");
                    }
                }
                Some(()) = self.file_change_rx.recv() => {
                    tracing::info!("File change detected, syncing file sources");
                    if let Err(err) = self.sync_and_refresh_file_sources(false).await {
                        tracing::error!(error = ?err, "Error during file source sync");
                    }
                }
                _ = shutdown.cancelled() => {
                    tracing::info!("Source loader shutting down");
                    break;
                }
            }
        }
    }

    /// Sync file sources with directories and refresh any that need it.
    /// On startup (`initial=true`), all file sources are refreshed.
    /// On file change events, only new/changed sources are refreshed.
    async fn sync_and_refresh_file_sources(&self, initial: bool) -> Result<()> {
        let discovered = scan_directories(&self.blocklist_dirs, &self.allowlist_dirs);

        // Build map of expected source IDs -> discovered files
        let mut expected: HashMap<String, &DiscoveredFile> = HashMap::new();
        for file in &discovered {
            let id = derive_source_id(&file.path, file.disposition);
            expected.insert(id, file);
        }

        // Get all existing file-managed sources from DB
        let all_sources: Vec<Source> = self.db.get_all_sources().collect().await;
        let existing_file_sources: Vec<&Source> =
            all_sources.iter().filter(|s| s.is_file_managed()).collect();

        let existing_ids: std::collections::HashSet<&str> = existing_file_sources
            .iter()
            .map(|s| s.id.as_str())
            .collect();

        // Delete sources whose files no longer exist on disk
        for source in &existing_file_sources {
            if !expected.contains_key(&source.id) {
                tracing::info!(source_id = source.id, "File source no longer has a file on disk, removing");
                if let Err(err) = self.db.delete_source(&source.id).await {
                    tracing::error!(source_id = source.id, error = ?err, "Failed to delete orphaned file source");
                }
            }
        }

        // Create sources for new files and collect IDs that need refresh
        let mut sources_to_refresh = Vec::new();

        for (id, file) in &expected {
            let is_new = !existing_ids.contains(id.as_str());

            if is_new {
                let now = Utc::now();
                let source = Source {
                    id: id.clone(),
                    url: None,
                    path: Some(file.path.to_string_lossy().to_string()),
                    disposition: file.disposition,
                    created_at: now,
                    updated_at: now,
                };
                tracing::info!(source_id = id.as_str(), path = %file.path.display(), "Creating new file source");
                if let Err(err) = self.db.put_source(source.clone()).await {
                    tracing::error!(source_id = id.as_str(), error = ?err, "Failed to create file source");
                    continue;
                }
                sources_to_refresh.push(source);
            } else if initial {
                // On startup, refresh all existing file sources
                if let Some(existing) = existing_file_sources.iter().find(|s| s.id == *id) {
                    // Update path in case it changed (e.g. directory was reconfigured)
                    let updated = Source {
                        path: Some(file.path.to_string_lossy().to_string()),
                        ..(*existing).clone()
                    };
                    sources_to_refresh.push(updated);
                }
            }
        }

        // On file change events (not initial), refresh ALL file sources since we
        // don't know which specific file changed (notify debounce batches events)
        if !initial && sources_to_refresh.is_empty() {
            // Even if no new sources, some file content may have changed
            for (id, file) in &expected {
                if existing_ids.contains(id.as_str())
                    && let Some(existing) = existing_file_sources.iter().find(|s| s.id == *id) {
                        let updated = Source {
                            path: Some(file.path.to_string_lossy().to_string()),
                            ..(*existing).clone()
                        };
                        sources_to_refresh.push(updated);
                    }
            }
        }

        if sources_to_refresh.is_empty() {
            return Ok(());
        }

        tracing::info!(count = sources_to_refresh.len(), "Refreshing file sources");

        let refresh_time = Utc::now();
        let mut any_success = false;

        for source in sources_to_refresh {
            match self.refresh(source.clone(), refresh_time).await {
                Ok(true) => {
                    any_success = true;
                    tracing::info!(source_id = source.id, "Successfully refreshed file source");
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::error!(source_id = source.id, error = ?err, "Failed to refresh file source");
                }
            }
        }

        if any_success {
            self.blocklist_authority.reload().await;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self), fields(source_id = source_id.as_str()))]
    async fn handle_manual_reload(&self, source_id: String) -> Result<()> {
        tracing::info!("Processing manual reload");

        // Fetch source from database
        let source = match self.db.get_source(&source_id).await {
            Ok(source) => source,
            Err(err) => {
                tracing::warn!(error = ?err, "Source not found");
                bail!("Source not found");
            }
        };

        // Verify source is auto-managed (has URL or path)
        if source.url.is_none() && source.path.is_none() {
            tracing::warn!("Source is manually-managed, cannot reload from URL/path");
            bail!("Source is manually-managed");
        }

        // Perform refresh
        let refresh_time = Utc::now();
        match self.refresh(source.clone(), refresh_time).await {
            Ok(true) => {
                tracing::info!("Manual reload complete");
                self.blocklist_authority.reload().await;
                Ok(())
            }
            Ok(false) => {
                tracing::info!("Reload lock held by another node, skipping manual reload");
                Ok(())
            }
            Err(err) => {
                tracing::error!(error = ?err, "Failed manual reload");
                Err(err)
            }
        }
    }

    async fn refresh_all(&self) -> Result<()> {
        tracing::info!("Checking for stale sources");

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

        tracing::info!(count = sources_to_refresh.len(), "Found stale sources to refresh");

        let mut successful_refreshes = 0;
        let mut failed_refreshes = 0;

        for source in sources_to_refresh {
            match self.refresh(source.clone(), start_time).await {
                Ok(true) => {
                    successful_refreshes += 1;
                    tracing::info!(source_id = source.id, "Successfully refreshed source");
                }
                Ok(false) => {}
                Err(err) => {
                    failed_refreshes += 1;
                    tracing::error!(source_id = source.id, error = ?err, "Failed to refresh source");
                }
            }
        }

        tracing::info!(successful = successful_refreshes, failed = failed_refreshes, "Source refresh cycle complete");

        if successful_refreshes > 0 {
            self.blocklist_authority.reload().await;
        }

        Ok(())
    }

    /// Refresh a single source. Returns `Ok(true)` if the refresh ran, `Ok(false)` if another
    /// node holds the per-source lock and the work was skipped.
    #[tracing::instrument(skip_all, fields(source_id = source.id.as_str()))]
    async fn refresh(&self, source: Source, refresh_time: DateTime<Utc>) -> Result<bool> {
        let lock_key = format!("blackhole:source-reload:{}", source.id);
        let lock = match self
            .lock_manager
            .lock(lock_key.as_bytes(), RELOAD_LOCK_TTL)
            .await
        {
            Ok(lock) => lock,
            Err(_) => {
                tracing::info!("Reload lock held by another node, skipping");
                return Ok(false);
            }
        };

        tracing::debug!("Refreshing source");

        let content = self
            .fetch_content(&source)
            .await
            .context("Failed to fetch source content")?;

        let parsed_hosts = crate::parser::parse_list(content)
            .await
            .context("Failed to parse source content")?;

        tracing::info!(host_count = parsed_hosts.len(), "Parsed hosts from source");

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

        tracing::debug!(stale_count = stale_hosts.len(), "Deleting stale hosts from source");

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

        self.lock_manager.unlock(&lock).await;
        Ok(true)
    }

    async fn fetch_content(
        &self,
        source: &Source,
    ) -> Result<Pin<Box<dyn AsyncBufRead + Send + Unpin>>> {
        if let Some(url) = &source.url {
            tracing::debug!(url, "Fetching source from URL");

            let response = self
                .http_client
                .get(url)
                .send()
                .await
                .with_context(|| format!("Failed to download from URL: {}", url))?;

            if !response.status().is_success() {
                bail!("HTTP request failed with status: {}", response.status());
            }

            let stream = response
                .bytes_stream()
                .map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)));

            Ok(Box::pin(BufReader::new(StreamReader::new(stream))))
        } else if let Some(path) = &source.path {
            tracing::debug!(path, "Loading source from file");

            let file = tokio::fs::File::open(path)
                .await
                .with_context(|| format!("Failed to open file: {}", path))?;

            Ok(Box::pin(BufReader::new(file)))
        } else {
            bail!("Source has neither URL nor path defined");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_source_id() {
        let path = PathBuf::from("/etc/blackhole/blocklist.d/ads.txt");
        assert_eq!(
            derive_source_id(&path, HostDisposition::Block),
            "file-block-ads-txt"
        );

        let path = PathBuf::from("/etc/blackhole/allowlist.d/personal.list");
        assert_eq!(
            derive_source_id(&path, HostDisposition::Allow),
            "file-allow-personal-list"
        );

        let path = PathBuf::from("/some/path/my--weird...file.txt");
        assert_eq!(
            derive_source_id(&path, HostDisposition::Block),
            "file-block-my-weird-file-txt"
        );
    }

    #[test]
    fn test_derive_source_id_truncation() {
        let long_name = "a".repeat(100) + ".txt";
        let path = PathBuf::from(format!("/etc/blackhole/blocklist.d/{}", long_name));
        let id = derive_source_id(&path, HostDisposition::Block);
        assert!(id.len() <= 64);
        assert!(id.starts_with("file-block-"));
    }
}
