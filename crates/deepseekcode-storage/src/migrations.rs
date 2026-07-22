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

    for migration in embedded::migrations::runner().get_migrations() {
        apply_migration(pool, migration, &applied_versions).await?;
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

fn split_trigger_statements(sql: &str) -> Vec<String> {
    sql.split("CREATE TRIGGER")
        .filter(|chunk| !chunk.trim().is_empty())
        .map(|chunk| format!("CREATE TRIGGER{chunk}"))
        .collect()
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
    Ok(true)
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
            version,
            "upgraded migration checksum to stable sha256 format"
        );
        return Ok(());
    }

    if migration_effects_present(pool, sql).await? {
        repair_checksum(pool, version, &expected).await?;
        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            version,
            stored_checksum = ?stored,
            expected_checksum = %expected,
            "migration checksum drift detected; repaired after schema verification"
        );
        return Ok(());
    }

    Err(DeepSeekCodeError::Storage(format!(
        "migration V{version} checksum mismatch (stored={stored:?}, expected={expected}) and schema verification failed"
    )))
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
        for statement in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if let Err(e) = sqlx::query(statement).execute(&mut *tx).await {
                let msg = e.to_string();
                if statement.to_ascii_uppercase().contains("ADD COLUMN")
                    && msg.contains("duplicate column name")
                {
                    continue;
                }
                tx.rollback().await.ok();
                return Err(DeepSeekCodeError::Storage(format!(
                    "migration V{version} ({}) failed: {e}",
                    migration.name()
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
    fn stable_checksum_is_line_ending_invariant() {
        let sql = "CREATE TABLE IF NOT EXISTS sessions (\n    id TEXT\n);\n";
        let crlf = sql.replace('\n', "\r\n");
        assert_eq!(
            stable_migration_checksum(sql),
            stable_migration_checksum(&crlf)
        );
    }

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

    #[tokio::test]
    async fn repairs_legacy_checksum_when_schema_present() {
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
}
