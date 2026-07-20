use deepseekcode_schema::permission::PermissionStatus;
use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::SqlitePool;
use std::collections::HashMap;

pub struct PermissionStore {
    pool: SqlitePool,
}

impl PermissionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load_all(&self) -> Result<HashMap<String, PermissionStatus>> {
        let rows = sqlx::query_as::<_, RuleRow>(
            r#"SELECT pattern, status FROM permissions WHERE status IN ('allowed', 'always', 'denied')"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|r| parse_status(&r.status).map(|s| (r.pattern, s)))
            .collect())
    }

    pub async fn save(
        &self,
        tool_name: &str,
        pattern: &str,
        status: PermissionStatus,
    ) -> Result<()> {
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT OR REPLACE INTO permissions (id, session_id, tool_name, pattern, status, created_at)
               VALUES (?, NULL, ?, ?, ?, ?)"#,
        )
        .bind(id)
        .bind(tool_name)
        .bind(pattern)
        .bind(format_status(status))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    pattern: String,
    status: String,
}

fn format_status(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Allowed => "allowed",
        PermissionStatus::Always => "always",
        PermissionStatus::Denied => "denied",
        PermissionStatus::Pending => "pending",
    }
}

fn parse_status(s: &str) -> Option<PermissionStatus> {
    match s {
        "allowed" => Some(PermissionStatus::Allowed),
        "always" => Some(PermissionStatus::Always),
        "denied" => Some(PermissionStatus::Denied),
        _ => None,
    }
}
