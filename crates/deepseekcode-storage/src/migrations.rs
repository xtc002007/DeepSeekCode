use deepseekcode_types::{DeepSeekCodeError, Result};
use refinery::Migration;
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

fn split_trigger_statements(sql: &str) -> Vec<String> {
    sql.split("CREATE TRIGGER")
        .filter(|chunk| !chunk.trim().is_empty())
        .map(|chunk| format!("CREATE TRIGGER{chunk}"))
        .collect()
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
        let expected = migration.checksum().to_string();
        if stored.as_deref() != Some(expected.as_str()) {
            return Err(DeepSeekCodeError::Storage(format!(
                "migration V{version} checksum mismatch (stored={stored:?}, expected={expected})"
            )));
        }
        return Ok(());
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
    .bind(migration.checksum().to_string())
    .execute(&mut *tx)
    .await
    .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

    Ok(())
}
