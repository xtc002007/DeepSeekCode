use chrono::{DateTime, Utc};
use deepseekcode_schema::policy::PolicyGrant;
use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::SqlitePool;

pub struct PolicyStore {
    pool: SqlitePool,
}

impl PolicyStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load_grants(&self, project_dir: &str) -> Result<Vec<PolicyGrant>> {
        // Normalize lookup key identically to save path so grants survive
        // restarts regardless of slash/case/verbatim variants.
        let key = crate::paths::normalize_directory_path_for_lookup(project_dir);
        // Also match the raw key for DBs written before normalization.
        let rows = sqlx::query_as::<_, GrantRow>(
            r#"SELECT id, scope, pattern, created_at FROM policy_grants
               WHERE (project_dir = ? OR project_dir = ?) AND action = 'permit'
               ORDER BY created_at ASC"#,
        )
        .bind(&key)
        .bind(project_dir)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_grant()).collect()
    }

    pub async fn save_grant(&self, project_dir: &str, scope: &str, pattern: &str) -> Result<()> {
        // Respect V7 UNIQUE(project_dir,scope,pattern): UPSERT instead of
        // random-id REPLACE which churned id/created_at every save.
        let key = crate::paths::normalize_directory_path_for_lookup(project_dir);
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO policy_grants (id, project_dir, scope, pattern, action, created_at)
               VALUES (?, ?, ?, ?, 'permit', ?)
               ON CONFLICT(project_dir, scope, pattern) DO UPDATE SET created_at=excluded.created_at"#,
        )
        .bind(id)
        .bind(&key)
        .bind(scope)
        .bind(pattern)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn delete_grant(&self, id: &str) -> Result<()> {
        sqlx::query(r#"DELETE FROM policy_grants WHERE id = ?"#)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Project-scoped delete: only removes the grant when it belongs to
    /// `project_dir` (both normalized and raw keys for legacy rows).
    /// Prevents cross-tenant deletion via enumerable IDs.
    pub async fn delete_grant_in_project(&self, id: &str, project_dir: &str) -> Result<()> {
        let key = crate::paths::normalize_directory_path_for_lookup(project_dir);
        let result = sqlx::query(
            r#"DELETE FROM policy_grants WHERE id = ? AND (project_dir = ? OR project_dir = ?)"#,
        )
        .bind(id)
        .bind(&key)
        .bind(project_dir)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(DeepSeekCodeError::NotFound(format!(
                "grant {id} not found in this project"
            )));
        }
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct GrantRow {
    id: String,
    scope: String,
    pattern: String,
    created_at: String,
}

impl GrantRow {
    fn into_grant(self) -> Result<PolicyGrant> {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(PolicyGrant {
            id: self.id,
            scope: self.scope,
            pattern: self.pattern,
            created_at,
        })
    }
}
