use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn open(path: &str) -> Result<Self> {
        // Accept both bare paths and `sqlite://` URLs (embedded vs server
        // callers previously disagreed; normalize here so both work).
        let normalized = normalize_sqlite_url(path);
        let options = SqliteConnectOptions::from_str(&normalized)
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(30))
            .foreign_keys(true);

        // In-memory DBs must share one connection: pool>1 gives isolated
        // `sqlite::memory:` databases where migrations are invisible.
        // Exact match (not `contains`) so a file named `foo:memory:.db`
        // is not misdetected.
        let is_memory = normalized == "sqlite::memory:"
            || normalized == "sqlite://:memory:"
            || normalized.ends_with(":memory:");
        let pool = if is_memory {
            SqlitePoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(30))
                .idle_timeout(Some(Duration::from_secs(60)))
                .connect_with(options)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
        } else {
            SqlitePoolOptions::new()
                .max_connections(8)
                .min_connections(1)
                .acquire_timeout(Duration::from_secs(30))
                .idle_timeout(Some(Duration::from_secs(300)))
                .connect_with(options)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
        };

        crate::migrations::run(&pool).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            "database opened"
        );
        Ok(Self { pool })
    }

    pub async fn open_in_memory() -> Result<Self> {
        // Single-connection private memory DB: no shared-cache filename
        // parsing pitfalls, migrations always visible (max_connections=1).
        Self::open("sqlite::memory:").await
    }
}

fn normalize_sqlite_url(path: &str) -> String {
    let p = path.trim();
    if p.starts_with("sqlite://") {
        return p.to_string();
    }
    // Preserve in-memory URLs verbatim (any `:memory:` variant).
    if p == ":memory:" || p == "sqlite::memory:" || p.contains(":memory:") {
        return "sqlite::memory:".to_string();
    }
    // sqlx accepts both `sqlite:path` and `sqlite://path`; prefer file URL.
    if p.starts_with("sqlite:") {
        return p.to_string();
    }
    // Bare Windows drive paths (`C:\a.db`, `C:/a.db`): use `sqlite:` prefix
    // so sqlx does not misparse the drive colon as a URL scheme.
    if p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic() {
        return format!("sqlite:{p}");
    }
    format!("sqlite://{p}")
}

impl Database {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
