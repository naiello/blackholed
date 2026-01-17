use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;

use futures::{Stream, StreamExt, TryStreamExt};
use sqlx::{sqlite::SqliteConnectOptions, Database, Executor, Pool, Row, Sqlite, SqlitePool};

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
    fn get_all_sources(&self) -> impl Stream<Item = Source>;
    async fn get_source(&self, id: &str) -> Result<Source>;
    async fn put_host(&self, host: SourceHost) -> Result<()>;
    async fn delete_host(&self, name: &str, source_id: &str) -> Result<()>;
    fn get_hosts_by_source(&self, source_id: &str) -> impl Stream<Item = SourceHost>;
    fn get_hosts_by_disposition(
        &self,
        disposition: HostDisposition,
    ) -> impl Stream<Item = SourceHost>;
    fn get_stale_hosts_by_source(
        &self,
        source_id: &str,
        updated_before: DateTime<Utc>,
    ) -> impl Stream<Item = SourceHost>;
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
                let disposition = HostDisposition::from_str(&disposition_str)
                    .ok_or_else(|| anyhow::anyhow!("Invalid disposition: {}", disposition_str))?;

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

    async fn get_source(&self, id: &str) -> Result<Source> {
        let row = sqlx::query(
            "SELECT id, url, path, disposition, created_at, updated_at FROM source WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .context("Failed to fetch source")?;

        let disposition_str: String = row.try_get("disposition")?;
        let disposition = HostDisposition::from_str(&disposition_str)
            .ok_or_else(|| anyhow::anyhow!("Invalid disposition: {}", disposition_str))?;

        Ok(Source {
            id: row.try_get("id")?,
            url: row.try_get("url")?,
            path: row.try_get("path")?,
            disposition,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    async fn put_host(&self, host: SourceHost) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO host (name, source_id, disposition, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(name, source_id) DO UPDATE SET
                disposition = excluded.disposition,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&host.name)
        .bind(&host.source_id)
        .bind(host.disposition.as_str())
        .bind(host.created_at)
        .bind(host.updated_at)
        .execute(&self.pool)
        .await
        .context("Failed to insert/update host")?;

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
                let disposition = HostDisposition::from_str(&disposition_str)
                    .ok_or_else(|| anyhow::anyhow!("Invalid disposition: {}", disposition_str))?;

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
                let disposition = HostDisposition::from_str(&disposition_str)
                    .ok_or_else(|| anyhow::anyhow!("Invalid disposition: {}", disposition_str))?;

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
                let disposition = HostDisposition::from_str(&disposition_str)
                    .ok_or_else(|| anyhow::anyhow!("Invalid disposition: {}", disposition_str))?;

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
