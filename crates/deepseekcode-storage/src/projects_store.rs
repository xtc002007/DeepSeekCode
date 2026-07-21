use chrono::Utc;
use deepseekcode_schema::project::Project;
use deepseekcode_types::{DeepSeekCodeError, Result};
use sqlx::SqlitePool;

use crate::paths::normalize_directory_path;

pub struct ProjectsStore {
    pool: SqlitePool,
}

impl ProjectsStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<Project>> {
        let rows = sqlx::query_as::<_, ProjectRow>(
            r#"SELECT path, last_opened_at FROM projects ORDER BY last_opened_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_project()).collect()
    }

    pub async fn upsert(&self, path: &str) -> Result<Project> {
        let path = normalize_directory_path(path);
        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO projects (path, last_opened_at) VALUES (?, ?)
               ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at"#,
        )
        .bind(&path)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        Ok(Project {
            path: path.to_string(),
            last_opened_at: now,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ProjectRow {
    path: String,
    last_opened_at: String,
}

impl ProjectRow {
    fn into_project(self) -> Result<Project> {
        Ok(Project {
            path: self.path,
            last_opened_at: chrono::DateTime::parse_from_rfc3339(&self.last_opened_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}
