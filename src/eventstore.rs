use std::{net::IpAddr, str::FromStr, time::Duration};

use crate::{
    config::EventStoreConfig,
    model::{BlockEvent, EventStoreBlockedEvent, EventStoreClient},
    types::Shared,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use hickory_server::proto::rr::LowerName;
use redis::AsyncCommands;
use tokio::{select, time};
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

#[async_trait]
pub trait EventStore: Shared {
    async fn put_block_event(&self, event: &BlockEvent) -> Result<()>;
    async fn get_clients(&self) -> Result<Vec<EventStoreClient>>;
    async fn get_clients_paginated(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreClient>>;
    async fn get_block_events_for_client(&self, ip: IpAddr) -> Result<Vec<EventStoreBlockedEvent>>;
    async fn get_block_events_for_client_paginated(
        &self,
        ip: IpAddr,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreBlockedEvent>>;
    async fn get_global_pause(&self) -> Result<Option<DateTime<Utc>>>;
    async fn get_client_pause(&self, ip: IpAddr) -> Result<Option<DateTime<Utc>>>;
    async fn put_global_pause(&self, expires: DateTime<Utc>) -> Result<()>;
    async fn put_client_pause(&self, ip: IpAddr, expires: DateTime<Utc>) -> Result<()>;
    async fn delete_global_pause(&self) -> Result<()>;
    async fn delete_client_pause(&self, ip: IpAddr) -> Result<()>;
}

pub struct RedisEventStore {
    conn: RedisEventStoreConnection,
    _sweeper: AbortOnDropHandle<()>,
}

impl Shared for RedisEventStore {}

#[derive(Clone)]
struct RedisEventStoreConnection {
    redis: redis::aio::MultiplexedConnection,
    client_ttl: TimeDelta,
}

impl Shared for RedisEventStoreConnection {}

struct RedisEventStoreSweeper {
    conn: RedisEventStoreConnection,
    event_ttl: TimeDelta,
    client_ttl: TimeDelta,
    interval: Duration,
}

impl RedisEventStore {
    pub async fn new(config: EventStoreConfig, shutdown: ShutdownGuard) -> Result<Self> {
        let client = redis::Client::open(config.endpoint.to_owned())
            .context("Failed to create Redis client")?;

        let conn = RedisEventStoreConnection {
            redis: client
                .get_multiplexed_async_connection()
                .await
                .context("Failed to open connection to Redis")?,
            client_ttl: config.client_ttl(),
        };

        let sweeper = RedisEventStoreSweeper {
            conn: conn.clone(),
            event_ttl: config.event_ttl(),
            client_ttl: config.client_ttl(),
            interval: config
                .sweeper_interval()
                .to_std()
                .context("Invalid Redis sweeper interval")?,
        };
        let sweeper_handle = shutdown.spawn_task_fn(|guard| async move {
            sweeper.run(guard).await;
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

    async fn get_clients_paginated(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreClient>> {
        let mut conn = self.redis.clone();

        // Get paginated results with scores, in reverse order (most recent first)
        let start = offset as isize;
        let stop = (offset + limit - 1) as isize;
        let results: Vec<(String, f64)> = conn
            .zrevrange_withscores("clients", start, stop)
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

    async fn get_block_events_for_client_paginated(
        &self,
        ip: IpAddr,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreBlockedEvent>> {
        let mut conn = self.redis.clone();
        let key = format!("blocked#{}", ip);

        // Get paginated results with scores, in reverse order (most recent first)
        let start = offset as isize;
        let stop = (offset + limit - 1) as isize;
        let results: Vec<(String, f64)> = conn
            .zrevrange_withscores(&key, start, stop)
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

    async fn put_global_pause(&self, expires: DateTime<Utc>) -> Result<()> {
        let mut conn = self.redis.clone();
        conn.set::<_, _, ()>("pause#global", expires.timestamp())
            .await
            .context("Failed to write global pause to Redis")
    }

    async fn get_global_pause(&self) -> Result<Option<DateTime<Utc>>> {
        let mut conn = self.redis.clone();
        let ts = conn
            .get::<_, Option<i64>>("pause#global")
            .await
            .context("Failed to get client pause from Redis")?
            .and_then(|ts| DateTime::from_timestamp(ts, 0));

        Ok(ts)
    }

    async fn delete_global_pause(&self) -> Result<()> {
        let mut conn = self.redis.clone();
        conn.del::<_, ()>("pause#global")
            .await
            .context("Failed to delete global pause from Redis")
    }

    async fn put_client_pause(&self, client: IpAddr, expires: DateTime<Utc>) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("pause#{client}");
        conn.set::<_, _, ()>(&key, expires.timestamp())
            .await
            .context("Failed to write global pause to Redis")?;

        conn.expire_at::<_, ()>(&key, expires.timestamp())
            .await
            .context("Failed to set client pause TTL")
    }

    async fn get_client_pause(&self, client: IpAddr) -> Result<Option<DateTime<Utc>>> {
        let mut conn = self.redis.clone();
        let key = format!("pause#{client}");
        let ts = conn
            .get::<_, Option<i64>>(key)
            .await
            .context("Failed to get client pause from Redis")?
            .and_then(|ts| DateTime::from_timestamp(ts, 0));

        Ok(ts)
    }

    async fn delete_client_pause(&self, client: IpAddr) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("pause#{client}");
        conn.del::<_, ()>(key)
            .await
            .context("Failed to delete global pause from Redis")
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

    async fn get_clients_paginated(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreClient>> {
        self.conn.get_clients_paginated(limit, offset).await
    }

    async fn get_block_events_for_client(&self, ip: IpAddr) -> Result<Vec<EventStoreBlockedEvent>> {
        self.conn.get_block_events_for_client(ip).await
    }

    async fn get_block_events_for_client_paginated(
        &self,
        ip: IpAddr,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<EventStoreBlockedEvent>> {
        self.conn
            .get_block_events_for_client_paginated(ip, limit, offset)
            .await
    }

    async fn put_global_pause(&self, expires: DateTime<Utc>) -> Result<()> {
        self.conn.put_global_pause(expires).await
    }

    async fn get_global_pause(&self) -> Result<Option<DateTime<Utc>>> {
        self.conn.get_global_pause().await
    }

    async fn delete_global_pause(&self) -> Result<()> {
        self.conn.delete_global_pause().await
    }

    async fn put_client_pause(&self, client: IpAddr, expires: DateTime<Utc>) -> Result<()> {
        self.conn.put_client_pause(client, expires).await
    }

    async fn get_client_pause(&self, client: IpAddr) -> Result<Option<DateTime<Utc>>> {
        self.conn.get_client_pause(client).await
    }

    async fn delete_client_pause(&self, client: IpAddr) -> Result<()> {
        self.conn.delete_client_pause(client).await
    }
}

impl RedisEventStoreSweeper {
    async fn run(mut self, shutdown: ShutdownGuard) {
        tracing::info!("Starting Redis sweeper task");
        let mut interval = time::interval(self.interval);
        loop {
            select! {
                _ = interval.tick() => {
                    if let Err(e) = self.sweep().await {
                        tracing::error!(error = %e, "Error during sweeper cleanup");
                    }
                },
                _ = shutdown.cancelled() => {
                    tracing::info!("Redis sweeper shutting down");
                    break;
                },
            }
        }
    }

    async fn sweep(&mut self) -> Result<()> {
        tracing::info!("Sweeping stale entries from Redis");

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
                        tracing::debug!(removed, client = %client.ip, "Removed stale events");
                        total_events_removed += removed;
                    }
                }
                Err(e) => {
                    tracing::error!(client = %client.ip, error = %e, "Failed to clean up blocked events");
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
                    tracing::debug!(removed, "Removed stale clients");
                    total_clients_removed = removed;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to clean up stale clients");
            }
        }

        tracing::info!(
            events_removed = total_events_removed,
            clients_removed = total_clients_removed,
            "Sweeper completed",
        );

        Ok(())
    }
}
