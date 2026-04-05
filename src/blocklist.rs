use std::{
    collections::{HashMap, HashSet},
    io, iter,
    net::IpAddr,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt, future::join_all};
use tokio::{
    select,
    sync::{RwLock, broadcast},
    time,
};

use hickory_server::{
    authority::{
        Authority, LookupControlFlow, LookupError, LookupObject, LookupOptions, MessageRequest,
        UpdateResult, ZoneType,
    },
    proto::{
        op::ResponseCode,
        rr::{LowerName, Name, RecordType},
    },
    server::RequestInfo,
};
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

use crate::eventstore::EventStore;
use crate::model::{BlockEvent, HostDisposition, SourceHost};
use crate::{config::BlocklistConfig, types::Shared};

pub struct BlocklistAuthority<BP: BlocklistProvider> {
    origin: LowerName,
    blocklist: RwLock<HashMap<LowerName, HostDisposition>>,
    wildcard_match: bool,
    min_wildcard_depth: u8,
    blocked_tx: broadcast::Sender<BlockEvent>,
    provider: Arc<BP>,
    global_pause_active: Arc<AtomicBool>,
    client_pause_active: Arc<RwLock<HashSet<IpAddr>>>,
    _event_logger: AbortOnDropHandle<()>,
    _pause_manager: AbortOnDropHandle<()>,
}

struct BlocklistAuthorityEventLogger<ES: EventStore> {
    eventstore: Arc<ES>,
    blocked_rx: broadcast::Receiver<BlockEvent>,
}

impl<BP: BlocklistProvider> BlocklistAuthority<BP> {
    pub async fn new<ES: EventStore + Shared>(
        origin: Name,
        config: &BlocklistConfig,
        provider: Arc<BP>,
        eventstore: Arc<ES>,
        shutdown: ShutdownGuard,
    ) -> Self {
        let (blocked_tx, blocked_rx) = broadcast::channel(1024);

        let mut event_logger = BlocklistAuthorityEventLogger {
            eventstore: eventstore.clone(),
            blocked_rx,
        };
        let event_logger_handle =
            shutdown.spawn_task_fn(|guard| async move { event_logger.run(guard).await });

        let global_pause_active = Arc::new(AtomicBool::new(false));
        let client_pause_active = Arc::new(RwLock::new(HashSet::new()));
        let pause_manager = PauseManager {
            eventstore: eventstore.clone(),
            global_pause_active: global_pause_active.clone(),
            client_pause_active: client_pause_active.clone(),
        };
        let pause_manager_handle =
            shutdown.spawn_task_fn(|guard| async move { pause_manager.run(guard).await });

        let authority = Self {
            origin: origin.into(),
            blocklist: RwLock::new(HashMap::new()),
            wildcard_match: config.wildcard_match,
            min_wildcard_depth: config.min_wildcard_depth,
            blocked_tx,
            provider,
            global_pause_active,
            client_pause_active,
            _event_logger: AbortOnDropHandle::new(event_logger_handle),
            _pause_manager: AbortOnDropHandle::new(pause_manager_handle),
        };

        authority.reload().await;

        authority
    }

    pub async fn reload(&self) {
        log::info!("Reloading blocklist");

        let mut blocked = self
            .provider
            .get_blocked_hosts()
            .filter_map(|host| async move {
                LowerName::from_str(&host.name)
                    .map(|name| (name, HostDisposition::Block))
                    .inspect_err(|err| log::error!("Skipping invalid blocklist host: {:?}", err))
                    .ok()
            })
            .collect::<HashMap<_, _>>()
            .await;

        let allowed = self
            .provider
            .get_allowed_hosts()
            .filter_map(|host| async move {
                LowerName::from_str(&host.name)
                    .map(|name| (name, HostDisposition::Allow))
                    .inspect_err(|err| log::error!("Skipping invalid allowlist host: {:?}", err))
                    .ok()
            })
            .collect::<HashMap<_, _>>()
            .await;

        log::info!(
            "Reload resulted in {} blocked, {} allowed hosts",
            blocked.len(),
            allowed.len()
        );

        blocked.extend(allowed);
        *self.blocklist.write().await = blocked;

        log::info!("Blocklist reload complete");
    }

    pub async fn reload_host(&self, name: &str) -> Result<()> {
        log::debug!("Reloading single host: {}", name);

        let lower_name = LowerName::from_str(name)
            .map_err(|err| anyhow::anyhow!("Invalid host name '{}': {:?}", name, err))?;

        let host_records: Vec<SourceHost> = self.provider.get_host(name).collect().await;

        let disposition = if host_records
            .iter()
            .any(|h| h.disposition == HostDisposition::Allow)
        {
            Some(HostDisposition::Allow)
        } else if !host_records.is_empty() {
            Some(HostDisposition::Block)
        } else {
            None
        };

        let mut blocklist = self.blocklist.write().await;
        match disposition {
            Some(d) => {
                blocklist.insert(lower_name.clone(), d);
            }
            None => {
                blocklist.remove(&lower_name);
            }
        }

        log::info!("Reloaded single host {lower_name}");
        Ok(())
    }

    pub fn set_global_pause(&self, is_paused: bool) {
        self.global_pause_active.store(is_paused, Ordering::Relaxed);
    }

    pub async fn set_client_pause(&self, client: IpAddr, is_paused: bool) {
        let mut client_pause_active = self.client_pause_active.write().await;
        if is_paused {
            client_pause_active.insert(client);
        } else {
            client_pause_active.remove(&client);
        }
    }

    fn wildcards(&self, host: &Name) -> Vec<LowerName> {
        host.iter()
            .enumerate()
            .filter_map(|(i, _x)| {
                if i > ((self.min_wildcard_depth - 1) as usize) {
                    Some(host.trim_to(i + 1).into_wildcard().into())
                } else {
                    None
                }
            })
            .rev()
            .collect()
    }

    async fn is_blocked(&self, name: &LowerName) -> bool {
        let mut match_list = vec![name.to_owned()];

        if self.wildcard_match {
            match_list.append(&mut self.wildcards(name));
        }

        log::trace!("match_list: {:?}", match_list);
        let blocklist = self.blocklist.read().await;
        let disposition = match_list.iter().find_map(|entry| blocklist.get(entry));
        matches!(disposition, Some(HostDisposition::Block))
    }

    async fn is_paused(&self, client: IpAddr) -> bool {
        self.global_pause_active.load(Ordering::Relaxed)
            || self.client_pause_active.read().await.contains(&client)
    }
}

#[async_trait]
impl<BP: BlocklistProvider + Sync + Send> Authority for BlocklistAuthority<BP> {
    type Lookup = BlocklistLookup;

    fn zone_type(&self) -> ZoneType {
        ZoneType::External
    }

    fn is_axfr_allowed(&self) -> bool {
        false
    }

    async fn update(&self, _update: &MessageRequest) -> UpdateResult<bool> {
        Err(ResponseCode::NotImp)
    }

    fn origin(&self) -> &LowerName {
        &self.origin
    }

    async fn lookup(
        &self,
        name: &LowerName,
        _rtype: RecordType,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        use LookupControlFlow::*;

        if !self.is_blocked(name).await {
            return Skip;
        }

        let nxdomain = LookupError::ResponseCode(ResponseCode::NXDomain);
        Break(Err(nxdomain))
    }

    async fn consult(
        &self,
        _name: &LowerName,
        _rtype: RecordType,
        _lookup_options: LookupOptions,
        last_result: LookupControlFlow<Box<dyn LookupObject>>,
    ) -> LookupControlFlow<Box<dyn LookupObject>> {
        last_result
    }

    async fn search(
        &self,
        request_info: RequestInfo<'_>,
        lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        use LookupControlFlow::*;

        let result = self
            .lookup(
                request_info.query.name(),
                request_info.query.query_type(),
                lookup_options,
            )
            .await;

        if matches!(result, Break(_)) {
            let event = BlockEvent {
                time: Utc::now(),
                src: request_info.src,
                name: request_info.query.name().clone(),
                record_type: request_info.query.query_type(),
            };

            self.blocked_tx
                .send(event)
                .inspect_err(|_| log::warn!("Failed to tx blocked event, no subscribers"))
                .ok();

            if self.is_paused(request_info.src.ip()).await {
                log::info!(
                    "Would have blocked query for {} ({}) from {}",
                    request_info.query.name(),
                    request_info.query.query_type(),
                    request_info.src.ip(),
                );
                return Skip;
            }

            log::info!(
                "Blocked query for {} ({}) from {}",
                request_info.query.name(),
                request_info.query.query_type(),
                request_info.src.ip(),
            );
        }

        result
    }

    async fn get_nsec_records(
        &self,
        _name: &LowerName,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        LookupControlFlow::Continue(Err(LookupError::from(io::Error::other(
            "Blocklist cannot serve NSEC records",
        ))))
    }
}

struct PauseManager<ES: EventStore> {
    eventstore: Arc<ES>,
    global_pause_active: Arc<AtomicBool>,
    client_pause_active: Arc<RwLock<HashSet<IpAddr>>>,
}

impl<ES: EventStore> PauseManager<ES> {
    async fn run(&self, shutdown: ShutdownGuard) {
        log::info!("Starting pause manager");

        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            select! {
                _ = interval.tick() => {
                    self.check_global_pause()
                        .await
                        .inspect_err(|err| log::error!("Failed to check global pause: {err}"))
                        .ok();

                    self.check_client_pauses()
                        .await
                        .inspect_err(|err| log::error!("Failed to check client pauses: {err}"))
                        .ok();
                },
                _ = shutdown.cancelled() => {
                    log::info!("Pause manager shutting down");
                    break;
                },
            }
        }
    }

    async fn check_global_pause(&self) -> Result<()> {
        let now = Utc::now();
        let is_paused = self
            .eventstore
            .get_global_pause()
            .await
            .context("Failed to read global pause")?
            .is_some_and(|ts| ts > now);
        self.global_pause_active.store(is_paused, Ordering::Relaxed);
        Ok(())
    }

    async fn check_client_pauses(&self) -> Result<()> {
        let now = Utc::now();
        let ips = self
            .eventstore
            .get_clients()
            .await
            .context("Failed to read clients from Redis")?
            .into_iter()
            .map(|client| async move {
                self.eventstore
                    .get_client_pause(client.ip)
                    .await
                    .inspect_err(|err| {
                        log::error!("Failed to inspect client pause for {}: {}", client.ip, err)
                    })
                    .ok()
                    .flatten()
                    .filter(|ts| ts > &now)
                    .map(|_| client.ip)
            });

        let paused_ips: HashSet<_> = join_all(ips).await.into_iter().flatten().collect();
        *self.client_pause_active.write().await = paused_ips;

        Ok(())
    }
}

pub struct BlocklistLookup {}

impl LookupObject for BlocklistLookup {
    fn is_empty(&self) -> bool {
        true
    }

    fn iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a hickory_server::proto::rr::Record> + Send + 'a> {
        Box::new(iter::empty())
    }

    fn take_additionals(&mut self) -> Option<Box<dyn LookupObject>> {
        None
    }
}

pub trait BlocklistProvider {
    fn get_blocked_hosts(&self) -> impl Stream<Item = SourceHost> + Send;
    fn get_allowed_hosts(&self) -> impl Stream<Item = SourceHost> + Send;
    fn get_host(&self, name: &str) -> impl Stream<Item = SourceHost> + Send;
}

impl<ES: EventStore> BlocklistAuthorityEventLogger<ES> {
    async fn run(&mut self, shutdown: ShutdownGuard) {
        log::info!("Starting BlocklistAuthority event persistence task");

        loop {
            select! {
                event = self.blocked_rx.recv() => {
                    match event {
                        Ok(event) => {
                            if let Err(e) = self.eventstore.put_block_event(&event).await {
                                log::error!("Failed to persist block event to EventStore: {}", e);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            log::warn!("Event persistence task lagged, skipped {} events", skipped);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            log::error!("Block event channel closed, stopping persistence task");
                            break;
                        }
                    }
                },
                _ = shutdown.cancelled() => {
                    log::info!("Event logger shutting down");
                    break;
                },
            }
        }
    }
}
