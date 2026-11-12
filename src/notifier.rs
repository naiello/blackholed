use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use futures::StreamExt;
use redis::AsyncCommands;
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

use crate::blocklist::{BlocklistAuthority, BlocklistProvider};
use crate::types::Shared;

const CHANNEL: &str = "blackholed:blocklist-reload";

/// Delay before resubscribing after the PubSub connection drops or fails to open
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

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
    // A ConnectionManager reconnects after Redis restarts; a bare connection
    // stays dead and every later publish fails
    conn: redis::aio::ConnectionManager,
    client: redis::Client,
}

impl BlocklistNotifier {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client =
            redis::Client::open(redis_url).context("Failed to create Redis client for notifier")?;
        let conn = client
            .get_connection_manager()
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
        // The ConnectionManager fails the first command after the connection drops
        // and reconnects behind it, so one retry covers a Redis restart
        if let Err(err) = conn.publish::<_, _, ()>(CHANNEL, &message).await {
            tracing::debug!(error = %err, "Publish failed, retrying once");
            conn.publish::<_, _, ()>(CHANNEL, &message)
                .await
                .context("Failed to publish blocklist change notification")?;
        }
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

            // The subscription ends when Redis restarts or the connection drops,
            // so keep resubscribing until shutdown
            let mut resubscribing = false;
            loop {
                match subscribe(&client).await {
                    Ok(pubsub) => {
                        if resubscribing {
                            tracing::info!("Resubscribed to blocklist reload channel");
                            // Notifications published while unsubscribed are lost; reload
                            // to catch up on blocklist changes (pauses are polled anyway)
                            blocklist.reload_quiet().await;
                        }

                        let mut stream = pubsub.into_on_message();
                        loop {
                            tokio::select! {
                                msg = stream.next() => match msg {
                                    Some(msg) => handle_message(&instance_id, &blocklist, msg).await,
                                    None => {
                                        tracing::warn!("Blocklist reload subscription closed, resubscribing");
                                        break;
                                    }
                                },
                                _ = guard.cancelled() => {
                                    tracing::info!("Blocklist reload subscriber shutting down");
                                    return;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        tracing::error!(error = ?err, "Failed to subscribe to blocklist reload channel");
                    }
                }

                resubscribing = true;
                tokio::select! {
                    _ = tokio::time::sleep(RESUBSCRIBE_DELAY) => {}
                    _ = guard.cancelled() => {
                        tracing::info!("Blocklist reload subscriber shutting down");
                        return;
                    }
                }
            }
        });

        AbortOnDropHandle::new(handle)
    }
}

async fn subscribe(client: &redis::Client) -> Result<redis::aio::PubSub> {
    let mut pubsub = client
        .get_async_pubsub()
        .await
        .context("Failed to create PubSub connection")?;
    pubsub
        .subscribe(CHANNEL)
        .await
        .context("Failed to subscribe to blocklist reload channel")?;
    Ok(pubsub)
}

async fn handle_message<BP>(instance_id: &str, blocklist: &BlocklistAuthority<BP>, msg: redis::Msg)
where
    BP: BlocklistProvider + Shared,
{
    let raw: String = match msg.get_payload() {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, "Failed to decode PubSub message");
            return;
        }
    };

    // Message format: "{instance_id}:{payload}"
    // Instance IDs contain "-" but not ":", so splitn(2) is safe.
    let mut parts = raw.splitn(2, ':');
    let (sender_id, payload) = match (parts.next(), parts.next()) {
        (Some(id), Some(p)) => (id, p),
        _ => {
            tracing::warn!(raw, "Malformed blocklist notification");
            return;
        }
    };

    if sender_id == instance_id {
        tracing::trace!("Skipping own blocklist notification");
        return;
    }

    tracing::info!(
        sender = sender_id,
        payload,
        "Received blocklist change notification"
    );

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
