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
        let rows = sqlx::query_as::<_, GrantRow>(
            r#"SELECT id, scope, pattern, created_at FROM policy_grants
               WHERE project_dir = ? AND action = 'permit'
               ORDER BY created_at ASC"#,
        )
        .bind(project_dir)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_grant()).collect()
    }

    pub async fn save_grant(&self, project_dir: &str, scope: &str, pattern: &str) -> Result<()> {
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT OR REPLACE INTO policy_grants (id, project_dir, scope, pattern, action, created_at)
               VALUES (?, ?, ?, ?, 'permit', ?)"#,
        )
        .bind(id)
        .bind(project_dir)
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
