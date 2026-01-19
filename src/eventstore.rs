use std::{net::IpAddr, str::FromStr, time::Duration};

use crate::model::{BlockEvent, EventStoreBlockedEvent, EventStoreClient};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use hickory_server::proto::rr::LowerName;
use redis::AsyncCommands;
use tokio::time;
use tokio_util::task::AbortOnDropHandle;

#[async_trait]
pub trait EventStore {
    async fn put_block_event(&self, event: &BlockEvent) -> Result<()>;
    async fn get_clients(&self) -> Result<Vec<EventStoreClient>>;
    async fn get_block_events_for_client(&self, ip: IpAddr) -> Result<Vec<EventStoreBlockedEvent>>;
}

pub struct RedisEventStoreConfig {
    pub endpoint: String,
    pub event_ttl: TimeDelta,
    pub client_ttl: TimeDelta,
    pub sweeper_interval: TimeDelta,
}

impl Default for RedisEventStoreConfig {
    fn default() -> Self {
        Self {
            endpoint: "redis://127.0.0.1:6379".to_string(),
            event_ttl: TimeDelta::hours(1),
            client_ttl: TimeDelta::days(1),
            sweeper_interval: TimeDelta::minutes(15),
        }
    }
}

pub struct RedisEventStore {
    conn: RedisEventStoreConnection,
    _sweeper: AbortOnDropHandle<()>,
}

#[derive(Clone)]
struct RedisEventStoreConnection {
    redis: redis::aio::MultiplexedConnection,
    client_ttl: TimeDelta,
}

struct RedisEventStoreSweeper {
    conn: RedisEventStoreConnection,
    event_ttl: TimeDelta,
    client_ttl: TimeDelta,
    interval: Duration,
}

impl RedisEventStore {
    pub async fn new(config: RedisEventStoreConfig) -> Result<Self> {
        let client =
            redis::Client::open(config.endpoint).context("Failed to create Redis client")?;
        let conn = RedisEventStoreConnection {
            redis: client
                .get_multiplexed_async_connection()
                .await
                .context("Failed to open connection to Redis")?,
            client_ttl: config.client_ttl,
        };

        let sweeper = RedisEventStoreSweeper {
            conn: conn.clone(),
            event_ttl: config.event_ttl,
            client_ttl: config.client_ttl,
            interval: config
                .sweeper_interval
                .to_std()
                .context("Invalid Redis sweeper interval")?,
        };
        let sweeper_handle = tokio::task::spawn(async move {
            sweeper.run().await;
        });

        let store = Self {
            conn,
            _sweeper: AbortOnDropHandle::new(sweeper_handle),
        };

        Ok(store)
    }
}

#[async_trait]
impl EventStore for RedisEventStoreConnection {
    async fn put_block_event(&self, event: &BlockEvent) -> Result<()> {
        let mut conn = self.redis.clone();
        let client = event.src.ip().to_string();
        let key = format!("blocked#{}", client);
        let member = event.name.to_string();
        let score = event.time.timestamp();

        conn.zadd::<_, _, _, ()>(&key, &member, score)
            .await
            .context("Failed to log event to blocked set")?;

        conn.zadd::<_, _, _, ()>("clients", &client, score)
            .await
            .context("Failed to add event to client set")?;

        conn.expire::<_, ()>(&key, self.client_ttl.num_seconds())
            .await
            .context("Failed to set client TTL")?;

        Ok(())
    }

    async fn get_clients(&self) -> Result<Vec<EventStoreClient>> {
        let mut conn = self.redis.clone();

        let results: Vec<(String, f64)> = conn
            .zrange_withscores("clients", 0, -1)
            .await
            .context("Failed to get clients from Redis")?;

        let clients = results
            .into_iter()
            .filter_map(|(ip_str, timestamp)| {
                let ip = ip_str.parse::<IpAddr>().ok()?;
                let last_seen = DateTime::from_timestamp(timestamp as i64, 0)?;
                Some(EventStoreClient { ip, last_seen })
            })
            .collect();

        Ok(clients)
    }

    async fn get_block_events_for_client(&self, ip: IpAddr) -> Result<Vec<EventStoreBlockedEvent>> {
        let mut conn = self.redis.clone();
        let key = format!("blocked#{}", ip);

        let results: Vec<(String, f64)> = conn
            .zrange_withscores(&key, 0, -1)
            .await
            .context("Failed to get block events from Redis")?;

        let events = results
            .into_iter()
            .filter_map(|(domain_str, timestamp)| {
                let name = LowerName::from_str(&domain_str).ok()?;
                let time = DateTime::from_timestamp(timestamp as i64, 0)?;
                Some(EventStoreBlockedEvent { name, time })
            })
            .collect();

        Ok(events)
    }
}

#[async_trait]
impl EventStore for RedisEventStore {
    async fn put_block_event(&self, event: &BlockEvent) -> Result<()> {
        self.conn.put_block_event(event).await
    }

    async fn get_clients(&self) -> Result<Vec<EventStoreClient>> {
        self.conn.get_clients().await
    }

    async fn get_block_events_for_client(&self, ip: IpAddr) -> Result<Vec<EventStoreBlockedEvent>> {
        self.conn.get_block_events_for_client(ip).await
    }
}

impl RedisEventStoreSweeper {
    async fn run(mut self) {
        log::info!("Starting Redis sweeper task");
        let mut interval = time::interval(self.interval);
        loop {
            interval.tick().await;

            if let Err(e) = self.sweep().await {
                log::error!("Error during sweeper cleanup: {}", e);
            }
        }
    }

    async fn sweep(&mut self) -> Result<()> {
        log::info!("Sweeping stale entries from Redis");

        let now = Utc::now().timestamp();
        let mut total_events_removed = 0;
        let mut total_clients_removed = 0;

        let clients = self
            .conn
            .get_clients()
            .await
            .context("Failed to retrieve clients")?;

        let event_cutoff = now - self.event_ttl.num_seconds();
        for client in clients {
            let key = format!("blocked#{}", client.ip);

            let rm_events = self
                .conn
                .redis
                .zrembyscore::<_, _, _, i64>(&key, f64::NEG_INFINITY, event_cutoff as f64)
                .await;

            match rm_events {
                Ok(removed) => {
                    if removed > 0 {
                        log::debug!("Removed {} stale event(s) from {}", removed, key);
                        total_events_removed += removed;
                    }
                }
                Err(e) => {
                    log::error!("Failed to clean up blocked events for {}: {}", client.ip, e);
                }
            }
        }

        let client_cutoff = now - self.client_ttl.num_seconds();
        let rm_clients = self
            .conn
            .redis
            .zrembyscore::<_, _, _, i64>("clients", f64::NEG_INFINITY, client_cutoff as f64)
            .await;

        match rm_clients {
            Ok(removed) => {
                if removed > 0 {
                    log::debug!("Removed {} stale client(s) from clients set", removed);
                    total_clients_removed = removed;
                }
            }
            Err(e) => {
                log::error!("Failed to clean up stale clients: {}", e);
            }
        }

        log::info!(
            "Sweeper completed: removed {} event(s) and {} client(s)",
            total_events_removed,
            total_clients_removed
        );

        Ok(())
    }
}
