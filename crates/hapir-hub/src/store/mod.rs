pub mod machines;
pub mod messages;
pub mod push_subscriptions;
pub mod sessions;
pub mod types;
pub mod users;
pub(super) mod versioned_updates;

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::fs::create_dir_all;
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

const SCHEMA_VERSION: i64 = 3;
const POOL_SIZE: u32 = 8;
const POOL_CONN_TIMEOUT: Duration = Duration::from_secs(5);

const REQUIRED_TABLES: &[&str] = &[
    "sessions",
    "machines",
    "messages",
    "users",
    "push_subscriptions",
];

pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

pub struct Store {
    pool: Pool<SqliteConnectionManager>,
}

impl Store {
    pub fn new(path: &str) -> Result<Self> {
        let db_path = Path::new(path);
        if let Some(dir) = db_path.parent() {
            create_dir_all(dir).with_context(|| {
                format!("failed to create database directory {}", dir.display())
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }

        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(POOL_CONN_TIMEOUT)
            .connection_customizer(Box::new(PragmaCustomizer))
            .build(manager)
            .context("failed to create connection pool")?;

        // Set file permissions on DB and WAL/SHM files
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in &["", "-wal", "-shm"] {
                let file_path = format!("{path}{suffix}");
                let _ =
                    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600));
            }
        }

        let store = Self { pool };
        store.initialize_schema()?;

        Ok(store)
    }

    pub fn conn(&self) -> PooledConn {
        self.pool.get().expect("failed to get pooled connection")
    }

    pub fn try_conn(&self) -> Result<PooledConn> {
        self.pool.get().context("connection pool timeout")
    }

    fn get_schema_version(&self) -> Result<i64> {
        let version: i64 = self
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("failed to read schema version")?;
        Ok(version)
    }

    fn set_schema_version(&self, version: i64) -> Result<()> {
        self.conn()
            .pragma_update(None, "user_version", version)
            .context("failed to set schema version")?;
        Ok(())
    }

    fn initialize_schema(&self) -> Result<()> {
        let current_version = self.get_schema_version()?;
        info!(
            current_version,
            target_version = SCHEMA_VERSION,
            "checking schema version"
        );

        if current_version == 0 {
            self.create_tables()?;
            self.set_schema_version(SCHEMA_VERSION)?;
            info!("created database schema v{SCHEMA_VERSION}");
            return Ok(());
        }

        if current_version < SCHEMA_VERSION {
            self.migrate_schema(current_version)?;
        }

        self.assert_required_tables()?;

        Ok(())
    }

    fn assert_required_tables(&self) -> Result<()> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1")
            .context("failed to prepare table check query")?;

        let missing: Vec<&str> = REQUIRED_TABLES
            .iter()
            .filter(|&&table| !stmt.exists(rusqlite::params![table]).unwrap_or(false))
            .copied()
            .collect();

        if !missing.is_empty() {
            anyhow::bail!(
                "SQLite schema is missing required tables ({}). \
                 Back up and rebuild the database, or run an offline migration to the expected schema version.",
                missing.join(", ")
            );
        }

        Ok(())
    }

    fn create_tables(&self) -> Result<()> {
        self.conn()
            .execute_batch(include_str!("schema.sql"))
            .context("failed to create tables")?;
        Ok(())
    }

    fn migrate_schema(&self, from_version: i64) -> Result<()> {
        let mut version = from_version;

        while version < SCHEMA_VERSION {
            info!(from = version, to = version + 1, "migrating schema");

            match version {
                // 该项目是对hapi的翻译
                // 不需要兼任旧版本hapi的迁移了，直接升到最新版本就行
                // 1 => self.migrate_v1_to_v2()?,
                // 2 => self.migrate_v2_to_v3()?,
                _ => {
                    warn!(version, "unknown schema version, skipping");
                }
            }

            version += 1;
            self.set_schema_version(version)?;
        }

        info!(version = SCHEMA_VERSION, "schema migration complete");
        Ok(())
    }
}

#[derive(Debug)]
struct PragmaCustomizer;

impl r2d2::CustomizeConnection<Connection, rusqlite::Error> for PragmaCustomizer {
    fn on_acquire(&self, conn: &mut Connection) -> std::result::Result<(), rusqlite::Error> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }
}
