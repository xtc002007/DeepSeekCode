use code_plus_plus_types::{CodePlusPlusError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub struct Database {
    pool: SqlitePool,
    /// Cached at open (after migrations): whether `session_input.admitted_seq`
    /// exists. Per-DB flag replaces the previous process-global `OnceLock`
    /// which leaked across DBs in tests (cross-DB pollution).
    has_admitted_seq: bool,
}

/// Acquire a dedicated SQLite connection already inside `BEGIN IMMEDIATE`.
/// `pool.begin()` is DEFERRED and does NOT serialize concurrent
/// `MAX(admitted_seq)+1` readers; IMMEDIATE takes the RESERVED lock up front
/// so a second writer blocks (busy-timeout) instead of reading the same MAX.
/// Retries on SQLITE_BUSY with backoff; caller MUST `COMMIT`/`ROLLBACK`.
pub async fn acquire_immediate(
    pool: &SqlitePool,
) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>> {
    use code_plus_plus_types::CodePlusPlusError;
    let mut attempt = 0u32;
    loop {
        let mut conn = pool.acquire().await.map_err(|e| {
            CodePlusPlusError::Storage(format!("acquire for BEGIN IMMEDIATE failed: {e}"))
        })?;
        match sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await {
            Ok(_) => return Ok(conn),
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                let busy = msg.contains("locked") || msg.contains("busy");
                if busy && attempt < 30 {
                    attempt += 1;
                    drop(conn);
                    tokio::time::sleep(Duration::from_millis(5 * attempt as u64)).await;
                    continue;
                }
                return Err(CodePlusPlusError::Storage(format!(
                    "BEGIN IMMEDIATE failed: {e}"
                )));
            }
        }
    }
}

pub(crate) async fn probe_has_admitted_seq(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
    )
    .fetch_one(pool)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false)
}

impl Database {
    pub async fn open(path: &str) -> Result<Self> {
        // Accept both bare paths and `sqlite://` URLs (embedded vs server
        // callers previously disagreed; normalize here so both work).
        let start = std::time::Instant::now();
        tracing::debug!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            operation = "database.open",
            path = %path,
            "storage op start"
        );
        let normalized = normalize_sqlite_url(path);
        let options = SqliteConnectOptions::from_str(&normalized)
            .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?
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
                .map_err(|e| {
                    tracing::error!(
                        target: code_plus_plus_telemetry::STORAGE,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        operation = "database.open",
                        path = %path,
                        error = %e,
                        "storage op failed"
                    );
                    CodePlusPlusError::Storage(e.to_string())
                })?
        } else {
            SqlitePoolOptions::new()
                .max_connections(8)
                .min_connections(1)
                .acquire_timeout(Duration::from_secs(30))
                .idle_timeout(Some(Duration::from_secs(300)))
                .connect_with(options)
                .await
                .map_err(|e| {
                    tracing::error!(
                        target: code_plus_plus_telemetry::STORAGE,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        operation = "database.open",
                        path = %path,
                        error = %e,
                        "storage op failed"
                    );
                    CodePlusPlusError::Storage(e.to_string())
                })?
        };

        crate::migrations::run(&pool).await.map_err(|e| {
            tracing::error!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                operation = "database.open",
                path = %path,
                error = %e,
                "storage migrations failed"
            );
            e
        })?;
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            operation = "database.open",
            path = %path,
            duration_ms = start.elapsed().as_millis() as u64,
            "database opened"
        );
        // Probe once per Database (not per process): migrations guarantee the
        // column on fresh DBs; legacy DBs are healed in `repair_legacy_backfills`.
        let has_admitted_seq = probe_has_admitted_seq(&pool).await;
        Ok(Self { pool, has_admitted_seq })
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

    /// Per-DB `admitted_seq` flag probed once at open (no global cache).
    pub fn has_admitted_seq(&self) -> bool {
        self.has_admitted_seq
    }
}
