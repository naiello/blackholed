use std::net::IpAddr;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use futures::StreamExt;
use redis::AsyncCommands;
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

use crate::blocklist::{BlocklistAuthority, BlocklistProvider};
use crate::types::Shared;

const CHANNEL: &str = "blackholed:blocklist-reload";

pub enum BlocklistNotification {
    Full,
    Host(String),
    GlobalPause(bool),
    ClientPause(IpAddr, bool),
}

impl BlocklistNotification {
    fn to_payload(&self) -> String {
        match self {
            Self::Full => "full".to_string(),
            Self::Host(name) => format!("host:{}", name),
            Self::GlobalPause(active) => format!("global-pause:{}", active),
            Self::ClientPause(ip, active) => format!("client-pause:{}:{}", ip, active),
        }
    }

    fn from_payload(payload: &str) -> Option<Self> {
        if payload == "full" {
            Some(Self::Full)
        } else if let Some(name) = payload.strip_prefix("host:") {
            Some(Self::Host(name.to_string()))
        } else if let Some(val) = payload.strip_prefix("global-pause:") {
            val.parse::<bool>().ok().map(Self::GlobalPause)
        } else if let Some(rest) = payload.strip_prefix("client-pause:") {
            // Format: "<ip>:<bool>" — split from the right to handle IPv6 addresses
            let sep = rest.rfind(':')?;
            let ip: IpAddr = rest[..sep].parse().ok()?;
            let active: bool = rest[sep + 1..].parse().ok()?;
            Some(Self::ClientPause(ip, active))
        } else {
            None
        }
    }
}

pub struct BlocklistNotifier {
    instance_id: String,
    conn: redis::aio::MultiplexedConnection,
    client: redis::Client,
}

impl BlocklistNotifier {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)
            .context("Failed to create Redis client for notifier")?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .context("Failed to open Redis connection for blocklist notifier")?;

        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let instance_id = format!("{}-{}", std::process::id(), nanos);

        tracing::info!(instance_id, "Blocklist notifier initialized");

        Ok(Self {
            instance_id,
            conn,
            client,
        })
    }

    pub async fn publish(&self, notification: BlocklistNotification) -> Result<()> {
        let mut conn = self.conn.clone();
        let payload = notification.to_payload();
        let message = format!("{}:{}", self.instance_id, payload);
        conn.publish::<_, _, ()>(CHANNEL, &message)
            .await
            .context("Failed to publish blocklist change notification")?;
        tracing::debug!(payload, "Published blocklist notification");
        Ok(())
    }

    pub fn spawn_subscriber<BP>(
        &self,
        blocklist: Arc<BlocklistAuthority<BP>>,
        shutdown: ShutdownGuard,
    ) -> AbortOnDropHandle<()>
    where
        BP: BlocklistProvider + Shared,
    {
        let instance_id = self.instance_id.clone();
        let client = self.client.clone();

        let handle = shutdown.spawn_task_fn(move |guard| async move {
            tracing::info!("Starting blocklist reload subscriber");

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(err) => {
                    tracing::error!(error = %err, "Failed to create PubSub connection");
                    return;
                }
            };

            if let Err(err) = pubsub.subscribe(CHANNEL).await {
                tracing::error!(error = %err, "Failed to subscribe to blocklist reload channel");
                return;
            }

            let mut stream = pubsub.on_message();

            loop {
                tokio::select! {
                    Some(msg) = stream.next() => {
                        let raw: String = match msg.get_payload() {
                            Ok(p) => p,
                            Err(err) => {
                                tracing::warn!(error = %err, "Failed to decode PubSub message");
                                continue;
                            }
                        };

                        // Message format: "{instance_id}:{payload}"
                        // Instance IDs contain "-" but not ":", so splitn(2) is safe.
                        let mut parts = raw.splitn(2, ':');
                        let (sender_id, payload) = match (parts.next(), parts.next()) {
                            (Some(id), Some(p)) => (id, p),
                            _ => {
                                tracing::warn!(raw, "Malformed blocklist notification");
                                continue;
                            }
                        };

                        if sender_id == instance_id {
                            tracing::trace!("Skipping own blocklist notification");
                            continue;
                        }

                        tracing::info!(sender = sender_id, payload, "Received blocklist change notification");

                        match BlocklistNotification::from_payload(payload) {
                            Some(BlocklistNotification::Full) => {
                                blocklist.reload_quiet().await;
                            }
                            Some(BlocklistNotification::Host(name)) => {
                                if let Err(err) = blocklist.reload_host_quiet(&name).await {
                                    tracing::error!(host = name, error = %err, "Failed to reload host from notification");
                                }
                            }
                            Some(BlocklistNotification::GlobalPause(active)) => {
                                blocklist.set_global_pause_quiet(active);
                            }
                            Some(BlocklistNotification::ClientPause(ip, active)) => {
                                blocklist.set_client_pause_quiet(ip, active).await;
                            }
                            None => {
                                tracing::warn!(payload, "Unknown blocklist notification payload");
                            }
                        }
                    }
                    _ = guard.cancelled() => {
                        tracing::info!("Blocklist reload subscriber shutting down");
                        break;
                    }
                }
            }
        });

        AbortOnDropHandle::new(handle)
    }
}
