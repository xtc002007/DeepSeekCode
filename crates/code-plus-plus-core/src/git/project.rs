use chrono::Utc;
use code_plus_plus_types::{CodePlusPlusError, Result};
use sqlx::SqlitePool;
use ulid::Ulid;

use std::path::{Path, PathBuf};

use super::worktree::discover_git_root;

/// Resolved on-disk layout for a Code++ project / location.
#[derive(Debug, Clone)]
pub struct ProjectDirectories {
    /// Active working directory (location scope).
    pub directory: PathBuf,
    /// Git worktree root when inside a repo, otherwise same as `directory`.
    pub project_root: PathBuf,
    /// `.code-plus-plus` under the working directory.
    pub code_plus_plus_dir: PathBuf,
    /// `.code-plus-plus/snapshots`
    pub snapshots_dir: PathBuf,
    /// `.code-plus-plus/tool-output`
    pub tool_output_dir: PathBuf,
}

impl ProjectDirectories {
    pub fn discover(directory: impl AsRef<Path>) -> Self {
        let directory = directory.as_ref().to_path_buf();
        let project_root = discover_git_root(&directory).unwrap_or_else(|| directory.clone());
        let code_plus_plus_dir = directory.join(".code-plus-plus");
        Self {
            snapshots_dir: code_plus_plus_dir.join("snapshots"),
            tool_output_dir: code_plus_plus_dir.join("tool-output"),
            code_plus_plus_dir,
            project_root,
            directory,
        }
    }

    pub fn relativize(&self, absolute: &Path) -> Option<String> {
        absolute
            .strip_prefix(&self.directory)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    }

    pub fn resolve(&self, relative: &str) -> PathBuf {
        let path = Path::new(relative);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.directory.join(path)
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectDirectoryEntry {
    pub id: String,
    pub project_path: String,
    pub directory_path: String,
    pub strategy: String,
    pub created_at: String,
}

/// SQLite-backed store for the `project_directories` table.
pub struct ProjectDirectoryStore {
    pool: SqlitePool,
}

impl ProjectDirectoryStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, project_path: &str) -> Result<Vec<ProjectDirectoryEntry>> {
        tracing::debug!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_path = %project_path,
            "project_directories list started"
        );
        let rows = sqlx::query_as::<_, ProjectDirectoryRow>(
            r#"SELECT id, project_path, directory_path, strategy, created_at
               FROM project_directories
               WHERE project_path = ?
               ORDER BY created_at ASC"#,
        )
        .bind(project_path)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;

        Ok(rows.into_iter().map(|r| r.into_entry()).collect())
    }

    pub async fn add(
        &self,
        project_path: &str,
        directory_path: &str,
        strategy: Option<&str>,
    ) -> Result<ProjectDirectoryEntry> {
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_path = %project_path,
            directory_path = %directory_path,
            "project_directories add started"
        );
        let id = Ulid::new().to_string();
        let now = Utc::now().to_rfc3339();
        let strategy = strategy.unwrap_or("default");

        sqlx::query(
            r#"INSERT INTO project_directories (id, project_path, directory_path, strategy, created_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(project_path, directory_path) DO UPDATE SET
                 strategy = excluded.strategy"#,
        )
        .bind(&id)
        .bind(project_path)
        .bind(directory_path)
        .bind(strategy)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;

        let row = sqlx::query_as::<_, ProjectDirectoryRow>(
            r#"SELECT id, project_path, directory_path, strategy, created_at
               FROM project_directories
               WHERE project_path = ? AND directory_path = ?"#,
        )
        .bind(project_path)
        .bind(directory_path)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;

        Ok(row.into_entry())
    }

    pub async fn remove(&self, project_path: &str, directory_path: &str) -> Result<()> {
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_path = %project_path,
            directory_path = %directory_path,
            "project_directories remove started"
        );
        sqlx::query(
            r#"DELETE FROM project_directories WHERE project_path = ? AND directory_path = ?"#,
        )
        .bind(project_path)
        .bind(directory_path)
        .execute(&self.pool)
        .await
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct ProjectDirectoryRow {
    id: String,
    project_path: String,
    directory_path: String,
    strategy: String,
    created_at: String,
}

impl ProjectDirectoryRow {
    fn into_entry(self) -> ProjectDirectoryEntry {
        ProjectDirectoryEntry {
            id: self.id,
            project_path: self.project_path,
            directory_path: self.directory_path,
            strategy: self.strategy,
            created_at: self.created_at,
        }
    }
}
