use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::{path::Path, str::FromStr};

use futures::{Stream, StreamExt, TryStreamExt};
use sqlx::{
    sqlite::SqliteConnectOptions, Database, Executor, Pool, QueryBuilder, Row, Sqlite, SqlitePool,
};

use crate::{
    blocklist::BlocklistProvider,
    model::{HostDisposition, Source, SourceHost},
};
use async_trait::async_trait;

pub struct SqlDb<DB: Database> {
    pool: Pool<DB>,
}

#[async_trait]
pub trait Db {
    async fn put_source(&self, source: Source) -> Result<()>;
    async fn delete_source(&self, id: &str) -> Result<()>;
    fn get_all_sources(&self) -> impl Stream<Item = Source> + Send;
    async fn get_all_sources_paginated(&self, limit: usize, offset: usize) -> Result<Vec<Source>>;
    async fn get_source(&self, id: &str) -> Result<Source>;
    async fn put_hosts(&self, hosts: Vec<SourceHost>) -> Result<()>;
    async fn delete_host(&self, name: &str, source_id: &str) -> Result<()>;
    fn get_hosts_by_source(&self, source_id: &str) -> impl Stream<Item = SourceHost> + Send;
    async fn get_hosts_by_source_paginated(
        &self,
        source_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SourceHost>>;
    fn get_hosts_by_disposition(
        &self,
        disposition: HostDisposition,
    ) -> impl Stream<Item = SourceHost> + Send;
    fn get_stale_hosts_by_source(
        &self,
        source_id: &str,
        updated_before: DateTime<Utc>,
    ) -> impl Stream<Item = SourceHost> + Send;
}

impl SqlDb<Sqlite> {
    pub async fn new_sqlite(path: impl AsRef<Path>) -> Result<SqlDb<Sqlite>> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts)
            .await
            .context("Failed to create Sqlite DB handle")?;

        init_db(&pool).await.context("Failed to init tables")?;

        Ok(SqlDb { pool })
    }
}

#[async_trait]
impl Db for SqlDb<Sqlite> {
    async fn put_source(&self, source: Source) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO source (id, url, path, disposition, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                url = excluded.url,
                path = excluded.path,
                disposition = excluded.disposition,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&source.id)
        .bind(&source.url)
        .bind(&source.path)
        .bind(source.disposition.as_str())
        .bind(source.created_at)
        .bind(source.updated_at)
        .execute(&self.pool)
        .await
        .context("Failed to insert/update source")?;

        Ok(())
    }

    async fn delete_source(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM host WHERE source_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete hosts for source")?;

        sqlx::query("DELETE FROM source WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete source")?;

        Ok(())
    }

    fn get_all_sources(&self) -> impl Stream<Item = Source> {
        sqlx::query("SELECT id, url, path, disposition, created_at, updated_at FROM source")
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move {
                let disposition_str: String = row.try_get("disposition")?;
                let disposition = HostDisposition::from_str(&disposition_str)?;

                Ok(Source {
                    id: row.try_get("id")?,
                    url: row.try_get("url")?,
                    path: row.try_get("path")?,
                    disposition,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .filter_map(|source| async move {
                source
                    .inspect_err(|err| log::error!("Invalid source entry in database: {:?}", err))
                    .ok()
            })
    }

    async fn get_all_sources_paginated(&self, limit: usize, offset: usize) -> Result<Vec<Source>> {
        let rows = sqlx::query(
            "SELECT id, url, path, disposition, created_at, updated_at FROM source LIMIT ? OFFSET ?",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .context("Failed to fetch sources")?;

        let mut sources = Vec::new();
        for row in rows {
            let disposition_str: String = row.try_get("disposition")?;
            let disposition = HostDisposition::from_str(&disposition_str)?;

            sources.push(Source {
                id: row.try_get("id")?,
                url: row.try_get("url")?,
                path: row.try_get("path")?,
                disposition,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            });
        }

        Ok(sources)
    }

    async fn get_source(&self, id: &str) -> Result<Source> {
        let row = sqlx::query(
            "SELECT id, url, path, disposition, created_at, updated_at FROM source WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .context("Failed to fetch source")?;

        let disposition_str: String = row.try_get("disposition")?;
        let disposition = HostDisposition::from_str(&disposition_str)?;

        Ok(Source {
            id: row.try_get("id")?,
            url: row.try_get("url")?,
            path: row.try_get("path")?,
            disposition,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    async fn put_hosts(&self, hosts: Vec<SourceHost>) -> Result<()> {
        if hosts.is_empty() {
            return Ok(());
        }

        for chunk in hosts.chunks(100) {
            let mut query_builder = QueryBuilder::new(
                "INSERT INTO host (name, source_id, disposition, created_at, updated_at) ",
            );

            query_builder.push_values(chunk, |mut b, host| {
                b.push_bind(&host.name)
                    .push_bind(&host.source_id)
                    .push_bind(host.disposition.as_str())
                    .push_bind(host.created_at)
                    .push_bind(host.updated_at);
            });

            query_builder.push(
                " ON CONFLICT(name, source_id) DO UPDATE SET \
                 disposition = excluded.disposition, \
                 updated_at = excluded.updated_at",
            );

            query_builder
                .build()
                .execute(&self.pool)
                .await
                .context("Failed to insert/update hosts")?;
        }

        Ok(())
    }

    async fn delete_host(&self, name: &str, source_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM host WHERE name = ? AND source_id = ?")
            .bind(name)
            .bind(source_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete host")?;

        Ok(())
    }

    fn get_hosts_by_source(&self, source_id: &str) -> impl Stream<Item = SourceHost> {
        let source_id = source_id.to_string();
        sqlx::query("SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ?")
            .bind(source_id)
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move {
                let disposition_str: String = row.try_get("disposition")?;
                let disposition = HostDisposition::from_str(&disposition_str)?;

                Ok(SourceHost {
                    name: row.try_get("name")?,
                    source_id: row.try_get("source_id")?,
                    disposition,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .filter_map(|host| async move {
                host.inspect_err(|err| log::error!("Invalid host entry in database: {:?}", err)).ok()
            })
    }

    async fn get_hosts_by_source_paginated(
        &self,
        source_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SourceHost>> {
        let rows = sqlx::query(
            "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ? LIMIT ? OFFSET ?",
        )
        .bind(source_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .context("Failed to fetch hosts")?;

        let mut hosts = Vec::new();
        for row in rows {
            let disposition_str: String = row.try_get("disposition")?;
            let disposition = HostDisposition::from_str(&disposition_str)?;

            hosts.push(SourceHost {
                name: row.try_get("name")?,
                source_id: row.try_get("source_id")?,
                disposition,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            });
        }

        Ok(hosts)
    }

    fn get_hosts_by_disposition(
        &self,
        disposition: HostDisposition,
    ) -> impl Stream<Item = SourceHost> {
        sqlx::query("SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE disposition = ?")
            .bind(disposition.as_str())
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move {
                let disposition_str: String = row.try_get("disposition")?;
                let disposition = HostDisposition::from_str(&disposition_str)?;

                Ok(SourceHost {
                    name: row.try_get("name")?,
                    source_id: row.try_get("source_id")?,
                    disposition,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .filter_map(|host| async move {
                host.inspect_err(|err| log::error!("Invalid host entry in database: {:?}", err)).ok()
            })
    }

    fn get_stale_hosts_by_source(
        &self,
        source_id: &str,
        updated_before: DateTime<Utc>,
    ) -> impl Stream<Item = SourceHost> {
        let source_id = source_id.to_string();
        sqlx::query("SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ? AND updated_at < ?")
            .bind(source_id)
            .bind(updated_before)
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move {
                let disposition_str: String = row.try_get("disposition")?;
                let disposition = HostDisposition::from_str(&disposition_str)?;

                Ok(SourceHost {
                    name: row.try_get("name")?,
                    source_id: row.try_get("source_id")?,
                    disposition,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .filter_map(|host| async move {
                host.inspect_err(|err| log::error!("Invalid host entry in database: {:?}", err)).ok()
            })
    }
}

impl BlocklistProvider for SqlDb<Sqlite> {
    fn get_blocked_hosts(&self) -> impl Stream<Item = SourceHost> {
        self.get_hosts_by_disposition(HostDisposition::Block)
    }

    fn get_allowed_hosts(&self) -> impl Stream<Item = SourceHost> {
        self.get_hosts_by_disposition(HostDisposition::Allow)
    }
}

async fn init_db<'e, E: Executor<'e> + Copy>(executor: E) -> Result<()> {
    sqlx::raw_sql(
        r#"
            CREATE TABLE IF NOT EXISTS source (
                id VARCHAR(24) PRIMARY KEY,
                url VARCHAR,
                path VARCHAR,
                disposition VARCHAR(16) NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );"#,
    )
    .execute(executor)
    .await
    .context("Failed to init source table")?;

    sqlx::raw_sql(
        r#"
            CREATE TABLE IF NOT EXISTS host (
                name VARCHAR(255) NOT NULL,
                source_id VARCHAR(24) NOT NULL,
                disposition VARCHAR(16) NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (name, source_id),
                FOREIGN KEY(source_id) REFERENCES source(id)
            );"#,
    )
    .execute(executor)
    .await
    .context("Failed to init host table")?;

    Ok(())
}
