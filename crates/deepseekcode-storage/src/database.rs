use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn open(path: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(path)
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        crate::migrations::run(&pool).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            path = path,
            "database opened"
        );
        Ok(Self { pool })
    }

    pub async fn open_in_memory() -> Result<Self> {
        Self::open("sqlite::memory:").await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
