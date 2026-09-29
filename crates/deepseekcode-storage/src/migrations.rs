use deepseekcode_types::{DeepSeekCodeError, Result};
use refinery::Migration;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("../../migrations");
}

const REFINERY_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS refinery_schema_history (
    version INTEGER PRIMARY KEY,
    name VARCHAR(255),
    applied_on VARCHAR(255) NOT NULL,
    checksum VARCHAR(255) NOT NULL
);
"#;

pub async fn run(pool: &SqlitePool) -> Result<()> {
    sqlx::query(REFINERY_TABLE)
        .execute(pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    let applied_versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM refinery_schema_history ORDER BY version")
            .fetch_all(pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    // Version-order application (critical): `embed_migrations!` yields files
    // in WalkDir filesystem order, NOT numeric version order (e.g. V10 before
    // V1 on NTFS). Applying out of order made backfill migrations (V9-V11)
    // run against missing tables; the old swallow-and-continue then recorded
    // them as applied, leaving afflicted DBs without backfill effects.
    let mut migrations = embedded::migrations::runner().get_migrations().clone();
    migrations.sort_by_key(|m| m.version());
    for migration in &migrations {
        apply_migration(pool, migration, &applied_versions).await?;
    }

    // Heal databases afflicted by the historic misordering: re-apply V10/V11
    // backfill effects idempotently (guarded by existence checks so fresh and
    // healthy DBs are untouched).
    repair_legacy_backfills(pool).await?;

    Ok(())
}

/// Idempotent repair for DBs whose V10/V11 ran out of order (and were then
/// recorded as applied with effects skipped). Every statement is guarded and
/// safe to re-run on healthy databases.
async fn repair_legacy_backfills(pool: &SqlitePool) -> Result<()> {
    // V11: ensure `admitted_seq` column exists (V11 may have been skipped).
    let has_seq: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
    )
    .fetch_one(pool)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    // Only touch session_input when the table itself exists.
    let has_input: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_input'"#,
    )
    .fetch_one(pool)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    if has_input && !has_seq {
        sqlx::query(
            r#"ALTER TABLE session_input ADD COLUMN admitted_seq INTEGER NOT NULL DEFAULT 0"#,
        )
        .execute(pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    }
    if has_input && has_seq {
        // Efficient backfill via window function (V11's correlated COUNT is
        // O(n^2)); only rows still at 0 are touched.
        let _ = sqlx::query(
            r#"UPDATE session_input SET admitted_seq = ranked.rn FROM (
                    SELECT rowid AS rid, ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY rowid) AS rn
                    FROM session_input WHERE admitted_seq = 0
                ) AS ranked WHERE session_input.rowid = ranked.rid"#,
        )
        .execute(pool)
        .await;
    }
    // V10: normalize legacy messages.seq = 0 (idempotent: only seq=0 rows).
    let has_messages: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages'"#,
    )
    .fetch_one(pool)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    if has_messages {
        let _ = sqlx::query(r#"UPDATE messages SET seq = seq + 1 WHERE seq = 0"#)
            .execute(pool)
            .await;
    }
    // V10: recompute sessions.next_message_seq where the column exists.
    let has_next_seq: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'next_message_seq'"#,
    )
    .fetch_one(pool)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    if has_next_seq && has_messages {
        let _ = sqlx::query(
            r#"UPDATE sessions SET next_message_seq = COALESCE(
                    (SELECT MAX(seq) + 1 FROM messages WHERE messages.session_id = sessions.id), 1)"#,
        )
        .execute(pool)
        .await;
    }
    Ok(())
}

fn normalize_migration_sql(sql: &str) -> String {
    sql.replace("\r\n", "\n").replace('\r', "\n")
}

fn stable_migration_checksum(sql: &str) -> String {
    let normalized = normalize_migration_sql(sql);
    let digest = Sha256::digest(normalized.as_bytes());
    format!("sha256:{digest:x}")
}

/// Split migration SQL into executable statements, aware of quotes,
/// line/block comments, backticks, and trigger bodies (`BEGIN..END`).
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut begin_depth: i32 = 0;
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let next = bytes.get(i + 1).map(|b| *b as char);
        // Comment handling (outside quotes).
        if !in_single && !in_double && !in_backtick {
            if !in_block_comment && c == '-' && next == Some('-') {
                in_line_comment = true;
                current.push(c);
                i += 1;
                continue;
            }
            if !in_line_comment && c == '/' && next == Some('*') {
                in_block_comment = true;
                current.push_str("/*");
                i += 2;
                continue;
            }
            if in_block_comment && c == '*' && next == Some('/') {
                in_block_comment = false;
                current.push_str("*/");
                i += 2;
                continue;
            }
            if in_line_comment && c == '\n' {
                in_line_comment = false;
            }
            if in_line_comment || in_block_comment {
                current.push(c);
                i += 1;
                continue;
            }
        }
        if in_line_comment || in_block_comment {
            current.push(c);
            i += 1;
            continue;
        }
        match c {
            '\'' if !in_double && !in_backtick => {
                // `''` is an escaped quote, not a toggle.
                if in_single && next == Some('\'') {
                    current.push_str("''");
                    i += 2;
                    continue;
                }
                in_single = !in_single;
                current.push(c);
                i += 1;
            }
            '"' if !in_single && !in_backtick => {
                in_double = !in_double;
                current.push(c);
                i += 1;
            }
            '`' if !in_single && !in_double => {
                in_backtick = !in_backtick;
                current.push(c);
                i += 1;
            }
            ';' if !in_single && !in_double && !in_backtick && begin_depth == 0 => {
                current.push(c);
                let stmt = current.trim().to_string();
                if !stmt.trim_matches(';').trim().is_empty() {
                    out.push(stmt);
                }
                current.clear();
                i += 1;
            }
            _ => {
                // Track BEGIN..END nesting for trigger bodies (case-insensitive,
                // word-boundary aware) so inner `;` are not split.
                if !in_single && !in_double && !in_backtick {
                    let rest_upper = sql[i..].to_ascii_uppercase();
                    let is_boundary = |len: usize| {
                        sql[i + len..]
                            .chars()
                            .next()
                            .map(|nc| !nc.is_ascii_alphanumeric() && nc != '_')
                            .unwrap_or(true)
                    };
                    if rest_upper.starts_with("BEGIN") && is_boundary(5) {
                        begin_depth += 1;
                    } else if rest_upper.starts_with("END") && is_boundary(3) {
                        begin_depth = (begin_depth - 1).max(0);
                    }
                }
                current.push(c);
                i += 1;
            }
        }
    }
    let tail = current.trim().to_string();
    if !tail.trim_matches(';').trim().is_empty() && !is_comment_only(&tail) {
        out.push(tail);
    }
    out.into_iter().filter(|s| !is_comment_only(s)).collect()
}

/// True when a chunk contains only `--` line comments, `/* */` blocks and
/// whitespace (no executable SQL). Such chunks are skipped instead of being
/// sent to SQLite.
fn is_comment_only(sql: &str) -> bool {
    let mut rest = sql.trim().trim_matches(';').trim();
    while !rest.is_empty() {
        if rest.starts_with("--") {
            match rest.find('\n') {
                Some(i) => rest = rest[i + 1..].trim(),
                None => return true,
            }
        } else if rest.starts_with("/*") {
            match rest.find("*/") {
                Some(i) => rest = rest[i + 2..].trim(),
                None => return true,
            }
        } else {
            return false;
        }
    }
    true
}

fn split_trigger_statements(sql: &str) -> Vec<String> {
    split_statements(sql)
}

async fn repair_checksum(pool: &SqlitePool, version: i32, checksum: &str) -> Result<()> {
    sqlx::query("UPDATE refinery_schema_history SET checksum = ? WHERE version = ?")
        .bind(checksum)
        .bind(version)
        .execute(pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    Ok(())
}

async fn verify_applied_migration(
    pool: &SqlitePool,
    migration: &Migration,
    stored: Option<String>,
) -> Result<()> {
    let version = migration.version() as i32;
    let sql = migration
        .sql()
        .ok_or_else(|| DeepSeekCodeError::Storage(format!("migration V{version} has no sql")))?;
    let expected = stable_migration_checksum(sql);
    let refinery_expected = migration.checksum().to_string();

    if stored.as_deref() == Some(expected.as_str()) {
        return Ok(());
    }

    if stored.as_deref() == Some(refinery_expected.as_str()) {
        repair_checksum(pool, version, &expected).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            version,
            "upgraded migration checksum to stable sha256 format"
        );
        return Ok(());
    }

    // Last-resort self-heal for databases written by older builds whose
    // embedded SQL drifted (CRLF/whitespace differences): if the migration's
    // schema effects are already present, the migration effectively ran —
    // repair the checksum instead of bricking the database on startup.
    if migration_effects_present(pool, sql).await? {
        repair_checksum(pool, version, &expected).await?;
        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            version,
            stored_checksum = ?stored,
            expected_checksum = %expected,
            "migration checksum drift detected; repaired after schema verification"
        );
        return Ok(());
    }

    Err(DeepSeekCodeError::Storage(format!(
        "migration V{version} checksum mismatch (stored={stored:?}, expected={expected}); \
         the database may come from an incompatible build — back it up and remove \
         refinery_schema_history rows for versions whose schema is already applied"
    )))
}

fn extract_create_tables(sql: &str) -> Vec<String> {
    let mut tables = Vec::new();
    let normalized = normalize_migration_sql(sql);
    for line in normalized.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        let prefix = if upper.starts_with("CREATE TABLE IF NOT EXISTS ") {
            "CREATE TABLE IF NOT EXISTS "
        } else if upper.starts_with("CREATE TABLE ") {
            "CREATE TABLE "
        } else {
            continue;
        };
        let rest = trimmed[prefix.len()..].trim();
        let name = rest
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .trim_matches('`')
            .trim_matches('[')
            .trim_matches(']');
        if !name.is_empty() {
            tables.push(name.to_string());
        }
    }
    tables
}

fn extract_add_columns(sql: &str) -> Vec<(String, String)> {
    let mut columns = Vec::new();
    let normalized = normalize_migration_sql(sql);
    for line in normalized.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        let upper = trimmed.to_ascii_uppercase();
        if !upper.starts_with("ALTER TABLE ") || !upper.contains(" ADD COLUMN ") {
            continue;
        }
        let rest = &trimmed["ALTER TABLE ".len()..];
        let Some((table_part, column_part)) = rest.split_once(" ADD COLUMN ") else {
            continue;
        };
        let table = table_part
            .trim()
            .trim_matches('"')
            .trim_matches('`')
            .to_string();
        let column = column_part
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .trim_matches('`')
            .to_string();
        if !table.is_empty() && !column.is_empty() {
            columns.push((table, column));
        }
    }
    columns
}

async fn table_exists(pool: &SqlitePool, table: &str) -> Result<bool> {
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1")
            .bind(table)
            .fetch_optional(pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    Ok(exists.is_some())
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    Ok(count > 0)
}

async fn index_exists(pool: &SqlitePool, index: &str) -> Result<bool> {
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ? LIMIT 1")
            .bind(index)
            .fetch_optional(pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    Ok(exists.is_some())
}

fn extract_create_indexes(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in split_statements(sql) {
        let upper = stmt.trim().to_ascii_uppercase();
        for prefix in [
            "CREATE UNIQUE INDEX IF NOT EXISTS ",
            "CREATE INDEX IF NOT EXISTS ",
            "CREATE UNIQUE INDEX ",
            "CREATE INDEX ",
        ] {
            if upper.starts_with(prefix) {
                let rest = stmt.trim()[prefix.len()..].trim();
                let name = rest
                    .split(|c: char| c.is_whitespace() || c == '(')
                    .next()
                    .unwrap_or_default()
                    .trim_matches(['"', '`', '[', ']'])
                    .to_string();
                if !name.is_empty() && !name.eq_ignore_ascii_case("ON") {
                    out.push(name);
                }
                break;
            }
        }
    }
    out
}

async fn migration_effects_present(pool: &SqlitePool, sql: &str) -> Result<bool> {
    for table in extract_create_tables(sql) {
        if !table_exists(pool, &table).await? {
            return Ok(false);
        }
    }
    for (table, column) in extract_add_columns(sql) {
        if !column_exists(pool, &table, &column).await? {
            return Ok(false);
        }
    }
    // Indexes / UNIQUE constraints must also exist, otherwise a drifted
    // checksum would be repaired while V7-style constraints are missing.
    for index in extract_create_indexes(sql) {
        if !index_exists(pool, &index).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn apply_migration(
    pool: &SqlitePool,
    migration: &Migration,
    applied_versions: &[i32],
) -> Result<()> {
    let version = migration.version() as i32;
    if applied_versions.contains(&version) {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT checksum FROM refinery_schema_history WHERE version = ?")
                .bind(version)
                .fetch_optional(pool)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        return verify_applied_migration(pool, migration, stored).await;
    }

    let sql = migration
        .sql()
        .ok_or_else(|| DeepSeekCodeError::Storage(format!("migration V{version} has no sql")))?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    if sql.to_ascii_uppercase().contains("CREATE TRIGGER") {
        for trigger in split_trigger_statements(sql) {
            if let Err(e) = sqlx::query(&trigger).execute(&mut *tx).await {
                tx.rollback().await.ok();
                return Err(DeepSeekCodeError::Storage(format!(
                    "migration V{version} ({}) failed: {e}",
                    migration.name()
                )));
            }
        }
    } else {
        for statement in split_statements(sql) {
            if let Err(e) = sqlx::query(&statement).execute(&mut *tx).await {
                // Fail-closed: no silent swallowing of `duplicate column` /
                // `no such table`. Idempotent migrations must use
                // `IF NOT EXISTS` in SQL; backfills must guard with
                // `SELECT COUNT(*) FROM sqlite_master` checks. Surfacing the
                // error prevents half-migrated databases from booting.
                tx.rollback().await.ok();
                return Err(DeepSeekCodeError::Storage(format!(
                    "migration V{version} ({}) failed: {e} [statement: {}]",
                    migration.name(),
                    statement.chars().take(200).collect::<String>()
                )));
            }
        }
    }

    sqlx::query(
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) VALUES (?, ?, ?, ?)",
    )
    .bind(version)
    .bind(migration.name())
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(stable_migration_checksum(sql))
    .execute(&mut *tx)
    .await
    .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_create_tables_and_columns() {
        let sql = r#"
CREATE TABLE IF NOT EXISTS sessions (id TEXT);
ALTER TABLE messages ADD COLUMN input_tokens INTEGER;
"#;
        assert_eq!(extract_create_tables(sql), vec!["sessions".to_string()]);
        assert_eq!(
            extract_add_columns(sql),
            vec![("messages".to_string(), "input_tokens".to_string())]
        );
    }

    #[test]
    fn embedded_migration_versions_are_unique_from_one() {
        // `embed_migrations!` yields WalkDir filesystem order (V10 before V1
        // on NTFS); `run()` sorts numerically before applying. This pins the
        // precondition that makes the sort meaningful: versions start at 1
        // with no duplicates (a duplicate would apply twice; a gap is fine).
        // The ordering guarantee itself is proven by every in-memory DB test,
        // which fails if backfills run against missing tables.
        let mut versions: Vec<u32> = embedded::migrations::runner()
            .get_migrations()
            .iter()
            .map(|m| m.version())
            .collect();
        assert!(!versions.is_empty());
        versions.sort_unstable();
        assert_eq!(versions[0], 1);
        for pair in versions.windows(2) {
            assert!(pair[0] < pair[1], "duplicate migration version");
        }
    }

    #[test]
    fn comment_only_chunks_are_skipped() {
        assert!(is_comment_only("-- just a comment\n-- another"));
        assert!(is_comment_only("/* block */"));
        assert!(!is_comment_only(
            "-- comment\nUPDATE messages SET seq = 1 WHERE seq = 0;"
        ));
    }

    #[test]
    fn stable_checksum_is_line_ending_invariant() {
        let sql = "CREATE TABLE IF NOT EXISTS sessions (\n    id TEXT\n);\n";
        let crlf = sql.replace('\n', "\r\n");
        assert_eq!(
            stable_migration_checksum(sql),
            stable_migration_checksum(&crlf)
        );
    }

    #[tokio::test]
    async fn repairs_legacy_refinery_checksum() {
        use crate::Database;

        let db = Database::open_in_memory().await.expect("memory db");
        let pool = db.pool().clone();

        sqlx::query("UPDATE refinery_schema_history SET checksum = ? WHERE version = 1")
            .bind("3550244922745839594")
            .execute(&pool)
            .await
            .unwrap();

        run(&pool).await.expect("migration repair should succeed");

        let stored: String =
            sqlx::query_scalar("SELECT checksum FROM refinery_schema_history WHERE version = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(stored.starts_with("sha256:"));
    }

    #[tokio::test]
    async fn repairs_drifted_checksum_when_schema_effects_present() {
        use crate::Database;

        let db = Database::open_in_memory().await.expect("memory db");
        let pool = db.pool().clone();

        // A checksum that matches neither the stable sha256 nor the legacy
        // refinery value, i.e. genuine drift. The schema itself is untouched,
        // so the effects-verification self-heal must repair it instead of
        // failing database open.
        sqlx::query("UPDATE refinery_schema_history SET checksum = ? WHERE version = 1")
            .bind("deadbeef-drifted")
            .execute(&pool)
            .await
            .unwrap();

        run(&pool).await.expect("drift repair should succeed");

        let stored: String =
            sqlx::query_scalar("SELECT checksum FROM refinery_schema_history WHERE version = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            stored.starts_with("sha256:"),
            "repaired checksum, got {stored}"
        );
    }
}
