use chrono::{DateTime, Utc};
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::git::ProjectDirectories;

pub const DELETED_MARKER: &str = ".deleted";

/// A captured on-disk copy of a file before a mutating tool runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub id: String,
    pub session_id: SessionId,
    pub message_id: Option<MessageId>,
    pub file_path: String,
    pub snapshot_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevertDiffEntry {
    pub path: String,
    pub snapshot_id: String,
    pub original_snapshot_id: Option<String>,
}

pub struct FileSnapshotStore {
    pool: SqlitePool,
    dirs: ProjectDirectories,
    enabled: bool,
}

impl FileSnapshotStore {
    pub fn new(pool: SqlitePool, project_dir: impl AsRef<Path>, enabled: bool) -> Self {
        Self {
            pool,
            dirs: ProjectDirectories::discover(project_dir),
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn directories(&self) -> &ProjectDirectories {
        &self.dirs
    }

    /// Capture the current contents of `file_path` (absolute) before mutation.
    pub async fn capture(
        &self,
        session_id: SessionId,
        message_id: Option<MessageId>,
        file_path: &Path,
    ) -> Result<FileSnapshot> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %session_id,
            path = %file_path.display(),
            "snapshot capture started"
        );
        if !self.enabled {
            return Err(DeepSeekCodeError::Other("snapshots are disabled".into()));
        }
        let relative = self
            .dirs
            .relativize(file_path)
            .ok_or_else(|| DeepSeekCodeError::Other("path outside project directory".into()))?;

        let id = ulid::Ulid::new().to_string();
        let session_dir = self.dirs.snapshots_dir.join(session_id.to_string());
        std::fs::create_dir_all(&session_dir).map_err(DeepSeekCodeError::Io)?;

        let file_name = format!(
            "{}_{}",
            id,
            Path::new(&relative)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".into())
        );
        let snapshot_path = session_dir.join(&file_name);

        if file_path.exists() && file_path.is_file() {
            std::fs::copy(file_path, &snapshot_path).map_err(DeepSeekCodeError::Io)?;
        } else {
            std::fs::write(&snapshot_path, DELETED_MARKER).map_err(DeepSeekCodeError::Io)?;
        }

        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO file_snapshots (id, session_id, message_id, file_path, snapshot_path, created_at)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(session_id.to_string())
        .bind(message_id.map(|m| m.to_string()))
        .bind(&relative)
        .bind(snapshot_path.to_string_lossy().as_ref())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                session_id = %session_id,
                error = %e,
                "snapshot capture failed: db insert"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;

        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %session_id,
            snapshot_id = %id,
            path = %relative,
            "snapshot capture completed"
        );
        Ok(FileSnapshot {
            id,
            session_id,
            message_id,
            file_path: relative,
            snapshot_path: snapshot_path.to_string_lossy().into_owned(),
            created_at: now,
        })
    }

    pub async fn get(&self, snapshot_id: &str) -> Result<Option<FileSnapshot>> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            snapshot_id = %snapshot_id,
            "snapshot get started"
        );
        let row = sqlx::query_as::<_, SnapshotRow>(
            r#"SELECT id, session_id, message_id, file_path, snapshot_path, created_at
               FROM file_snapshots WHERE id = ?"#,
        )
        .bind(snapshot_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        row.map(|r| r.into_snapshot()).transpose()
    }

    /// Snapshots for tool results tied to messages after `boundary_seq`.
    pub async fn snapshots_after_seq(
        &self,
        session_id: SessionId,
        boundary_seq: i64,
    ) -> Result<Vec<FileSnapshot>> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %session_id,
            boundary_seq = boundary_seq,
            "snapshots_after_seq started"
        );
        let rows = sqlx::query_as::<_, SnapshotRow>(
            r#"SELECT fs.id, fs.session_id, fs.message_id, fs.file_path, fs.snapshot_path, fs.created_at
               FROM file_snapshots fs
               LEFT JOIN messages m ON fs.message_id = m.id
               WHERE fs.session_id = ?
                 AND (m.seq > ? OR (fs.message_id IS NULL AND fs.created_at > (
                     SELECT created_at FROM messages WHERE session_id = ? AND seq = ? LIMIT 1
                 )))
                -- Insertion order (rowid), not wall-clock strings: clock
                -- skew or same-second captures otherwise misorder reverts.
                ORDER BY fs.rowid ASC"#,
        )
        .bind(session_id.to_string())
        .bind(boundary_seq)
        .bind(session_id.to_string())
        .bind(boundary_seq)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_snapshot()).collect()
    }

    /// Restore `target` from a stored snapshot. Deletes the file when the snapshot is a deletion marker.
    pub fn restore_snapshot(snapshot_path: &Path, target: &Path) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            snapshot = %snapshot_path.display(),
            target = %target.display(),
            "snapshot restore started"
        );
        // Exact-byte match: a text snapshot merely CONTAINING ".deleted"
        // (even with different whitespace) must restore, not delete. Binary
        // snapshots fall through to the byte-copy path with errors surfaced.
        let is_deletion = std::fs::read(snapshot_path)
            .map(|bytes| bytes == DELETED_MARKER.as_bytes())
            .unwrap_or(false);
        if is_deletion {
            if target.exists() {
                if target.is_dir() {
                    std::fs::remove_dir_all(target).map_err(DeepSeekCodeError::Io)?;
                } else {
                    std::fs::remove_file(target).map_err(DeepSeekCodeError::Io)?;
                }
            }
            return Ok(());
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(DeepSeekCodeError::Io)?;
        }
        std::fs::copy(snapshot_path, target).map_err(DeepSeekCodeError::Io)?;
        Ok(())
    }

    pub fn restore_entry(&self, entry: &FileSnapshot) -> Result<()> {
        let target = self.dirs.resolve(&entry.file_path);
        Self::restore_snapshot(Path::new(&entry.snapshot_path), &target)
    }

    /// First snapshot per file path after the boundary (earliest pre-mutation state per file).
    pub fn earliest_per_file(snapshots: Vec<FileSnapshot>) -> Vec<FileSnapshot> {
        let mut by_path: std::collections::BTreeMap<String, FileSnapshot> =
            std::collections::BTreeMap::new();
        for snap in snapshots {
            by_path.entry(snap.file_path.clone()).or_insert(snap);
        }
        by_path.into_values().collect()
    }

    pub fn is_write_tool(name: &str) -> bool {
        is_mutating_tool(name)
    }

    pub fn paths_from_tool_input(
        tool_name: &str,
        input: &Value,
        working_directory: &str,
    ) -> Vec<PathBuf> {
        mutating_tool_paths(tool_name, input, working_directory)
    }
}

/// Extract filesystem paths from mutating builtin tool arguments.
pub fn mutating_tool_paths(
    tool_name: &str,
    input: &Value,
    working_directory: &str,
) -> Vec<PathBuf> {
    let base = PathBuf::from(working_directory);
    let resolve = |rel: &str| {
        let p = PathBuf::from(rel);
        if p.is_absolute() {
            p
        } else {
            base.join(p)
        }
    };

    match tool_name {
        "write" | "edit" => input["path"].as_str().map(resolve).into_iter().collect(),
        "bash" | "powershell" => {
            let command = input["command"].as_str().unwrap_or("");
            crate::project_context::shell_command_target_paths(command, working_directory)
        }
        "apply_patch" => {
            let patch = input["patch"].as_str().unwrap_or("");
            let mut paths = Vec::new();
            for line in patch.lines() {
                // Accept `+++ b/path`, `+++ path`, and quoted forms; skip
                // `/dev/null` (new-file deletions have no snapshot).
                let rel = line
                    .strip_prefix("+++ b/")
                    .or_else(|| line.strip_prefix("+++ a/"))
                    .or_else(|| line.strip_prefix("+++ "))
                    .map(str::trim)
                    .map(|s| s.trim_matches('"').trim())
                    .unwrap_or("");
                if rel.is_empty() || rel == "/dev/null" || rel == "b/dev/null" {
                    continue;
                }
                // Only treat it as a path header (not patch content).
                if line.starts_with("+++") {
                    paths.push(resolve(rel));
                }
            }
            paths
        }
        _ => Vec::new(),
    }
}

pub fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "write" | "edit" | "apply_patch" | "bash" | "powershell"
    )
}

#[derive(sqlx::FromRow)]
struct SnapshotRow {
    id: String,
    session_id: String,
    message_id: Option<String>,
    file_path: String,
    snapshot_path: String,
    created_at: String,
}

impl SnapshotRow {
    fn into_snapshot(self) -> Result<FileSnapshot> {
        Ok(FileSnapshot {
            id: self.id,
            session_id: SessionId::from_str(&self.session_id)
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
            message_id: self
                .message_id
                .map(|id| MessageId::from_str(&id))
                .transpose()
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
            file_path: self.file_path,
            snapshot_path: self.snapshot_path,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn mutating_tool_paths_extracts_write_path() {
        let paths = mutating_tool_paths("write", &json!({"path": "src/main.rs"}), "/proj");
        assert_eq!(paths, vec![PathBuf::from("/proj/src/main.rs")]);
    }

    #[test]
    fn mutating_tool_paths_extracts_patch_files() {
        let patch = "--- a/foo.rs\n+++ b/foo.rs\n--- a/bar.rs\n+++ b/bar.rs\n";
        let paths = mutating_tool_paths("apply_patch", &json!({"patch": patch}), "/proj");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn mutating_tool_paths_extracts_bash_redirect() {
        let paths = mutating_tool_paths(
            "bash",
            &json!({"command": "echo hello > src/out.txt"}),
            "/proj",
        );
        assert_eq!(paths, vec![PathBuf::from("/proj/src/out.txt")]);
    }

    #[tokio::test]
    async fn capture_and_restore_roundtrip() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "before").unwrap();

        let db = deepseekcode_storage::Database::open_in_memory()
            .await
            .unwrap();
        let store = FileSnapshotStore::new(db.pool().clone(), dir.path(), true);
        let session_id = SessionId::new();

        let snap = store.capture(session_id, None, &file).await.unwrap();
        std::fs::write(&file, "after").unwrap();
        store.restore_entry(&snap).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    }
}
