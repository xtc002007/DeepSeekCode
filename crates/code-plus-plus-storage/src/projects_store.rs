use chrono::Utc;
use code_plus_plus_schema::project::Project;
use code_plus_plus_types::{CodePlusPlusError, Result};
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
        let start = std::time::Instant::now();
        tracing::debug!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            operation = "projects.list",
            "storage op start"
        );
        let rows = sqlx::query_as::<_, ProjectRow>(
            r#"SELECT path, last_opened_at FROM projects ORDER BY last_opened_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                operation = "projects.list",
                error = %e,
                "storage op failed"
            );
            CodePlusPlusError::Storage(e.to_string())
        })?;

        let out: Result<Vec<Project>> = rows.into_iter().map(|r| r.into_project()).collect();
        match &out {
            Ok(projects) => tracing::debug!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                operation = "projects.list",
                duration_ms = start.elapsed().as_millis() as u64,
                rows = projects.len(),
                "storage op ok"
            ),
            Err(e) => tracing::error!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                operation = "projects.list",
                error = %e,
                "storage op failed"
            ),
        }
        out
    }

    pub async fn upsert(&self, path: &str) -> Result<Project> {
        let start = std::time::Instant::now();
        tracing::debug!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            operation = "projects.upsert",
            project_dir = %path,
            "storage op start"
        );
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
        .map_err(|e| {
            tracing::error!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                operation = "projects.upsert",
                project_dir = %path,
                error = %e,
                "storage op failed"
            );
            CodePlusPlusError::Storage(e.to_string())
        })?;

        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            operation = "projects.upsert",
            project_dir = %path,
            duration_ms = start.elapsed().as_millis() as u64,
            "storage op ok"
        );
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
