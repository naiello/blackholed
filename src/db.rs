use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

use futures::{Stream, StreamExt, TryStreamExt};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};

use crate::{
    blocklist::BlocklistProvider,
    config::{DatabaseConfig, DbDriver},
    model::{HostDisposition, Source, SourceHost},
    types::Shared,
};
use async_trait::async_trait;

pub struct SqlDb {
    pool: AnyPool,
    driver: DbDriver,
}

#[async_trait]
pub trait Db: Shared {
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
    fn get_host(&self, name: &str) -> impl Stream<Item = SourceHost> + Send;
    fn get_stale_hosts_by_source(
        &self,
        source_id: &str,
        updated_before: DateTime<Utc>,
    ) -> impl Stream<Item = SourceHost> + Send;
}

impl SqlDb {
    pub async fn new(config: &DatabaseConfig) -> Result<SqlDb> {
        sqlx::any::install_default_drivers();

        let connect_url = match config.driver {
            DbDriver::Sqlite => format!("sqlite://{}?mode=rwc", config.url),
            DbDriver::Postgres => config.url.clone(),
        };
        let driver = config.driver;

        let pool = AnyPoolOptions::new()
            .connect(&connect_url)
            .await
            .context("Failed to create database pool")?;

        init_db(&pool, driver)
            .await
            .context("Failed to init tables")?;

        Ok(SqlDb { pool, driver })
    }
}

impl Shared for SqlDb {}

/// Format a DateTime<Utc> as RFC3339 for storage as TEXT in the database.
fn fmt_ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Parse a timestamp string (RFC3339 or SQLite's space-separated format) into DateTime<Utc>.
fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // SQLite CURRENT_TIMESTAMP format: "2024-01-01 00:00:00"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt.and_utc());
    }
    bail!("Failed to parse timestamp: {}", s)
}

fn parse_source_row(row: &sqlx::any::AnyRow) -> Result<Source> {
    let disposition_str: String = row.try_get("disposition")?;
    let disposition = disposition_str.parse()?;
    let created_at_str: String = row.try_get("created_at")?;
    let updated_at_str: String = row.try_get("updated_at")?;
    Ok(Source {
        id: row.try_get("id")?,
        url: row.try_get("url")?,
        path: row.try_get("path")?,
        disposition,
        created_at: parse_ts(&created_at_str)?,
        updated_at: parse_ts(&updated_at_str)?,
    })
}

fn parse_host_row(row: &sqlx::any::AnyRow) -> Result<SourceHost> {
    let disposition_str: String = row.try_get("disposition")?;
    let disposition = disposition_str.parse()?;
    let created_at_str: String = row.try_get("created_at")?;
    let updated_at_str: String = row.try_get("updated_at")?;
    Ok(SourceHost {
        name: row.try_get("name")?,
        source_id: row.try_get("source_id")?,
        disposition,
        created_at: parse_ts(&created_at_str)?,
        updated_at: parse_ts(&updated_at_str)?,
    })
}

#[async_trait]
impl Db for SqlDb {
    async fn put_source(&self, source: Source) -> Result<()> {
        let sql = match self.driver {
            DbDriver::Sqlite => {
                r#"
                INSERT INTO source (id, url, path, disposition, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    url = excluded.url,
                    path = excluded.path,
                    disposition = excluded.disposition,
                    updated_at = excluded.updated_at"#
            }
            DbDriver::Postgres => {
                r#"
                INSERT INTO source (id, url, path, disposition, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT(id) DO UPDATE SET
                    url = excluded.url,
                    path = excluded.path,
                    disposition = excluded.disposition,
                    updated_at = excluded.updated_at"#
            }
        };
        sqlx::query(sql)
            .bind(&source.id)
            .bind(&source.url)
            .bind(&source.path)
            .bind(source.disposition.as_str())
            .bind(fmt_ts(source.created_at))
            .bind(fmt_ts(source.updated_at))
            .execute(&self.pool)
            .await
            .context("Failed to insert/update source")?;

        Ok(())
    }

    async fn delete_source(&self, id: &str) -> Result<()> {
        let (delete_hosts_sql, delete_source_sql) = match self.driver {
            DbDriver::Sqlite => (
                "DELETE FROM host WHERE source_id = ?",
                "DELETE FROM source WHERE id = ?",
            ),
            DbDriver::Postgres => (
                "DELETE FROM host WHERE source_id = $1",
                "DELETE FROM source WHERE id = $1",
            ),
        };

        sqlx::query(delete_hosts_sql)
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete hosts for source")?;

        sqlx::query(delete_source_sql)
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
            .and_then(|row| async move { parse_source_row(&row) })
            .filter_map(|source| async move {
                source
                    .inspect_err(|err| tracing::error!(error = ?err, "Invalid source entry in database"))
                    .ok()
            })
    }

    async fn get_all_sources_paginated(&self, limit: usize, offset: usize) -> Result<Vec<Source>> {
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT id, url, path, disposition, created_at, updated_at FROM source LIMIT ? OFFSET ?"
            }
            DbDriver::Postgres => {
                "SELECT id, url, path, disposition, created_at, updated_at FROM source LIMIT $1 OFFSET $2"
            }
        };
        let rows = sqlx::query(sql)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch sources")?;

        rows.iter().map(parse_source_row).collect()
    }

    async fn get_source(&self, id: &str) -> Result<Source> {
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT id, url, path, disposition, created_at, updated_at FROM source WHERE id = ?"
            }
            DbDriver::Postgres => {
                "SELECT id, url, path, disposition, created_at, updated_at FROM source WHERE id = $1"
            }
        };
        let row = sqlx::query(sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .context("Failed to fetch source")?;

        parse_source_row(&row)
    }

    async fn put_hosts(&self, hosts: Vec<SourceHost>) -> Result<()> {
        if hosts.is_empty() {
            return Ok(());
        }

        // Deduplicate by (name, source_id) — Postgres rejects an ON CONFLICT statement
        // that targets the same row twice within a single INSERT.
        let mut seen = std::collections::HashSet::new();
        let hosts: Vec<SourceHost> = hosts
            .into_iter()
            .filter(|h| seen.insert((h.name.clone(), h.source_id.clone())))
            .collect();

        for chunk in hosts.chunks(100) {
            let values = match self.driver {
                DbDriver::Sqlite => {
                    let tuple = "(?, ?, ?, ?, ?)";
                    let mut s = String::with_capacity(chunk.len() * (tuple.len() + 2));
                    for i in 0..chunk.len() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        s.push_str(tuple);
                    }
                    s
                }
                DbDriver::Postgres => {
                    let mut s = String::with_capacity(chunk.len() * 25);
                    for i in 0..chunk.len() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        let b = i * 5 + 1;
                        use std::fmt::Write;
                        write!(s, "(${b}, ${}, ${}, ${}, ${})", b + 1, b + 2, b + 3, b + 4)
                            .unwrap();
                    }
                    s
                }
            };
            let sql = format!(
                "INSERT INTO host (name, source_id, disposition, created_at, updated_at) \
                 VALUES {} \
                 ON CONFLICT(name, source_id) DO UPDATE SET \
                     disposition = excluded.disposition, \
                     updated_at = excluded.updated_at",
                values
            );

            let mut query = sqlx::query(&sql);
            for host in chunk {
                query = query
                    .bind(&host.name)
                    .bind(&host.source_id)
                    .bind(host.disposition.as_str())
                    .bind(fmt_ts(host.created_at))
                    .bind(fmt_ts(host.updated_at));
            }
            query
                .execute(&self.pool)
                .await
                .context("Failed to insert/update hosts")?;
        }

        Ok(())
    }

    async fn delete_host(&self, name: &str, source_id: &str) -> Result<()> {
        let sql = match self.driver {
            DbDriver::Sqlite => "DELETE FROM host WHERE name = ? AND source_id = ?",
            DbDriver::Postgres => "DELETE FROM host WHERE name = $1 AND source_id = $2",
        };
        sqlx::query(sql)
            .bind(name)
            .bind(source_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete host")?;

        Ok(())
    }

    fn get_hosts_by_source(&self, source_id: &str) -> impl Stream<Item = SourceHost> {
        let source_id = source_id.to_string();
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ?"
            }
            DbDriver::Postgres => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = $1"
            }
        };
        sqlx::query(sql)
            .bind(source_id)
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move { parse_host_row(&row) })
            .filter_map(|host| async move {
                host.inspect_err(|err| tracing::error!(error = ?err, "Invalid host entry in database"))
                    .ok()
            })
    }

    async fn get_hosts_by_source_paginated(
        &self,
        source_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SourceHost>> {
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ? LIMIT ? OFFSET ?"
            }
            DbDriver::Postgres => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = $1 LIMIT $2 OFFSET $3"
            }
        };
        let rows = sqlx::query(sql)
            .bind(source_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch hosts")?;

        rows.iter().map(parse_host_row).collect()
    }

    fn get_hosts_by_disposition(
        &self,
        disposition: HostDisposition,
    ) -> impl Stream<Item = SourceHost> {
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE disposition = ?"
            }
            DbDriver::Postgres => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE disposition = $1"
            }
        };
        sqlx::query(sql)
            .bind(disposition.as_str())
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move { parse_host_row(&row) })
            .filter_map(|host| async move {
                host.inspect_err(|err| tracing::error!(error = ?err, "Invalid host entry in database"))
                    .ok()
            })
    }

    fn get_stale_hosts_by_source(
        &self,
        source_id: &str,
        updated_before: DateTime<Utc>,
    ) -> impl Stream<Item = SourceHost> {
        let source_id = source_id.to_string();
        let updated_before_str = fmt_ts(updated_before);
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = ? AND updated_at < ?"
            }
            DbDriver::Postgres => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE source_id = $1 AND updated_at < $2"
            }
        };
        sqlx::query(sql)
            .bind(source_id)
            .bind(updated_before_str)
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move { parse_host_row(&row) })
            .filter_map(|host| async move {
                host.inspect_err(|err| tracing::error!(error = ?err, "Invalid host entry in database"))
                    .ok()
            })
    }

    fn get_host(&self, name: &str) -> impl Stream<Item = SourceHost> {
        let name = name.to_string();
        let sql = match self.driver {
            DbDriver::Sqlite => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE name = ?"
            }
            DbDriver::Postgres => {
                "SELECT name, source_id, disposition, created_at, updated_at FROM host WHERE name = $1"
            }
        };
        sqlx::query(sql)
            .bind(name)
            .fetch(&self.pool)
            .map_err(anyhow::Error::from)
            .and_then(|row| async move { parse_host_row(&row) })
            .filter_map(|host| async move {
                host.inspect_err(|err| tracing::error!(error = ?err, "Invalid host entry in database"))
                    .ok()
            })
    }
}

impl BlocklistProvider for SqlDb {
    fn get_blocked_hosts(&self) -> impl Stream<Item = SourceHost> {
        self.get_hosts_by_disposition(HostDisposition::Block)
    }

    fn get_allowed_hosts(&self) -> impl Stream<Item = SourceHost> {
        self.get_hosts_by_disposition(HostDisposition::Allow)
    }

    fn get_host(&self, name: &str) -> impl Stream<Item = SourceHost> {
        Db::get_host(self, name)
    }
}

async fn init_db(pool: &AnyPool, driver: DbDriver) -> Result<()> {
    let sql = match driver {
        DbDriver::Sqlite => {
            r#"
            CREATE TABLE IF NOT EXISTS source (
                id VARCHAR(24) PRIMARY KEY,
                url VARCHAR,
                path VARCHAR,
                disposition VARCHAR(16) NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS host (
                name VARCHAR(255) NOT NULL,
                source_id VARCHAR(24) NOT NULL,
                disposition VARCHAR(16) NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (name, source_id),
                FOREIGN KEY(source_id) REFERENCES source(id)
            );"#
        }
        DbDriver::Postgres => {
            r#"
            CREATE TABLE IF NOT EXISTS source (
                id VARCHAR(24) PRIMARY KEY,
                url VARCHAR,
                path VARCHAR,
                disposition VARCHAR(16) NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS host (
                name VARCHAR(255) NOT NULL,
                source_id VARCHAR(24) NOT NULL,
                disposition VARCHAR(16) NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (name, source_id),
                FOREIGN KEY(source_id) REFERENCES source(id)
            );"#
        }
    };
    sqlx::raw_sql(sql)
        .execute(pool)
        .await
        .context("Failed to init tables")?;

    Ok(())
}
