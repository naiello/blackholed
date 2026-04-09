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
}

impl BlocklistNotification {
    fn to_payload(&self) -> String {
        match self {
            Self::Full => "full".to_string(),
            Self::Host(name) => format!("host:{}", name),
        }
    }

    fn from_payload(payload: &str) -> Option<Self> {
        if payload == "full" {
            Some(Self::Full)
        } else {
            payload
                .strip_prefix("host:")
                .map(|name| Self::Host(name.to_string()))
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

        log::info!(
            "Blocklist notifier initialized with instance ID: {}",
            instance_id
        );

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
        log::debug!("Published blocklist notification: {}", payload);
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
            log::info!("Starting blocklist reload subscriber");

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(err) => {
                    log::error!("Failed to create PubSub connection: {}", err);
                    return;
                }
            };

            if let Err(err) = pubsub.subscribe(CHANNEL).await {
                log::error!(
                    "Failed to subscribe to blocklist reload channel: {}",
                    err
                );
                return;
            }

            let mut stream = pubsub.on_message();

            loop {
                tokio::select! {
                    Some(msg) = stream.next() => {
                        let raw: String = match msg.get_payload() {
                            Ok(p) => p,
                            Err(err) => {
                                log::warn!("Failed to decode PubSub message: {}", err);
                                continue;
                            }
                        };

                        // Message format: "{instance_id}:{payload}"
                        // Instance IDs contain "-" but not ":", so splitn(2) is safe.
                        let mut parts = raw.splitn(2, ':');
                        let (sender_id, payload) = match (parts.next(), parts.next()) {
                            (Some(id), Some(p)) => (id, p),
                            _ => {
                                log::warn!("Malformed blocklist notification: {}", raw);
                                continue;
                            }
                        };

                        if sender_id == instance_id {
                            log::trace!("Skipping own blocklist notification");
                            continue;
                        }

                        log::info!(
                            "Received blocklist change notification from {}: {}",
                            sender_id,
                            payload
                        );

                        match BlocklistNotification::from_payload(payload) {
                            Some(BlocklistNotification::Full) => {
                                blocklist.reload_quiet().await;
                            }
                            Some(BlocklistNotification::Host(name)) => {
                                if let Err(err) = blocklist.reload_host_quiet(&name).await {
                                    log::error!(
                                        "Failed to reload host {} from notification: {}",
                                        name,
                                        err
                                    );
                                }
                            }
                            None => {
                                log::warn!(
                                    "Unknown blocklist notification payload: {}",
                                    payload
                                );
                            }
                        }
                    }
                    _ = guard.cancelled() => {
                        log::info!("Blocklist reload subscriber shutting down");
                        break;
                    }
                }
            }
        });

        AbortOnDropHandle::new(handle)
    }
}
