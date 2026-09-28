use deepseekcode_schema::permission::PermissionStatus;
use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Legacy compatibility store for the pre-policy allow/ask/deny system.
///
/// Deprecated for new code: project-scoped governance lives in
/// `PolicyStore` (SQLite `policy_grants` plus in-memory rulesets).
/// This table intentionally stays global and session-agnostic so old
/// `permission/*` API wrappers keep working for one release cycle;
/// do not add project scoping here, migrate callers instead.
pub struct PermissionStore {
    pool: SqlitePool,
}

impl PermissionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load_all(&self) -> Result<HashMap<String, PermissionStatus>> {
        // Legacy global read (session_id IS NULL, any project). New code must
        // use PolicyStore (project-scoped); this remains for one release
        // cycle compat only.
        let rows = sqlx::query_as::<_, RuleRow>(
            r#"SELECT pattern, status, created_at FROM permissions WHERE status IN ('allowed', 'always', 'denied') AND session_id IS NULL ORDER BY created_at ASC"#,
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
        // Deduplicate (tool_name,pattern): random-id REPLACE never deduped and
        // piled duplicates. Delete-then-insert keeps one live row.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(
            r#"DELETE FROM permissions WHERE tool_name = ? AND pattern = ? AND session_id IS NULL"#,
        )
        .bind(tool_name)
        .bind(pattern)
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO permissions (id, session_id, tool_name, pattern, status, created_at)
               VALUES (?, NULL, ?, ?, ?, ?)"#,
        )
        .bind(id)
        .bind(tool_name)
        .bind(pattern)
        .bind(format_status(status))
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    pattern: String,
    status: String,
    #[allow(dead_code)]
    created_at: String,
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
