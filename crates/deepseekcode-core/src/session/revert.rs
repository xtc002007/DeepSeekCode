use chrono::Utc;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::session::{RevertFileEntry, RevertState};
use deepseekcode_storage::{EventStore, SessionStore};
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result, SessionId};
use serde_json::json;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;

use crate::snapshot::{FileSnapshotStore, RevertDiffEntry};

pub type StagedRevert = RevertState;

const SHELL_ONLY_REVERT_WARNING: &str =
    "Timeline reverted only; workspace files were not restored because shell commands had no capturable paths.";

pub struct RevertManager {
    snapshots: FileSnapshotStore,
    sessions: SessionStore,
    pool: SqlitePool,
    events: Arc<EventStore>,
}

impl RevertManager {
    pub fn new(
        project_dir: String,
        pool: SqlitePool,
        sessions: SessionStore,
        events: Arc<EventStore>,
    ) -> Self {
        let snapshots = FileSnapshotStore::new(pool.clone(), project_dir, true);
        Self {
            snapshots,
            sessions,
            pool,
            events,
        }
    }

    pub async fn get_staged(&self, session_id: SessionId) -> Result<Option<StagedRevert>> {
        let row = sqlx::query_as::<_, RevertRow>(
            r#"SELECT staged_message_id, staged_at, diff_json FROM session_revert WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        row.map(|r| r.into_state()).transpose()
    }

    pub async fn stage(
        &self,
        session_id: SessionId,
        message_id: MessageId,
    ) -> Result<StagedRevert> {
        if self.get_staged(session_id).await?.is_some() {
            return Err(DeepSeekCodeError::Other(
                "revert already staged; call clear or commit first".into(),
            ));
        }

        let boundary_seq = self.message_seq(session_id, message_id).await?;
        let candidates = self
            .snapshots
            .snapshots_after_seq(session_id, boundary_seq)
            .await?;
        let to_restore = FileSnapshotStore::earliest_per_file(candidates);

        let mut diff_entries = Vec::new();
        for entry in &to_restore {
            let target = self.snapshots.directories().resolve(&entry.file_path);
            let original = self
                .snapshots
                .capture(session_id, Some(message_id), &target)
                .await?;
            diff_entries.push(RevertDiffEntry {
                path: entry.file_path.clone(),
                snapshot_id: entry.id.clone(),
                original_snapshot_id: Some(original.id),
            });
            self.snapshots.restore_entry(entry)?;
        }

        let shell_only = self
            .has_non_revertable_shell_after_seq(session_id, boundary_seq)
            .await?;
        let warning = shell_only.then(|| SHELL_ONLY_REVERT_WARNING.to_string());

        let staged_at = Utc::now();
        let diff_json = json!({
            "files": diff_entries,
            "warning": warning,
        });
        sqlx::query(
            r#"INSERT INTO session_revert (session_id, staged_message_id, staged_at, diff_json)
               VALUES (?, ?, ?, ?)"#,
        )
        .bind(session_id.to_string())
        .bind(message_id.to_string())
        .bind(staged_at.to_rfc3339())
        .bind(diff_json.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        Ok(StagedRevert {
            staged_message_id: message_id,
            staged_at,
            files: diff_entries
                .into_iter()
                .map(|e| RevertFileEntry {
                    path: e.path,
                    snapshot_id: e.snapshot_id,
                })
                .collect(),
            warning,
        })
    }

    pub async fn clear(&self, session_id: SessionId) -> Result<()> {
        let Some(state) = self.get_staged(session_id).await? else {
            return Ok(());
        };

        let diff: serde_json::Value = sqlx::query_scalar::<_, String>(
            r#"SELECT diff_json FROM session_revert WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
        .map(|s| serde_json::from_str(&s).unwrap_or(json!({})))
        .unwrap_or(json!({}));

        if let Some(files) = diff.get("files").and_then(|f| f.as_array()) {
            for file in files {
                if let Some(original_id) = file.get("original_snapshot_id").and_then(|v| v.as_str())
                {
                    if let Some(snap) = self.snapshots.get(original_id).await? {
                        self.snapshots.restore_entry(&snap)?;
                    }
                }
            }
        }

        sqlx::query(r#"DELETE FROM session_revert WHERE session_id = ?"#)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        let _ = state;
        Ok(())
    }

    pub async fn commit(&self, session_id: SessionId) -> Result<StagedRevert> {
        let Some(state) = self.get_staged(session_id).await? else {
            return Err(DeepSeekCodeError::Other(
                "no staged revert to commit".into(),
            ));
        };

        let boundary_seq = self
            .message_seq(session_id, state.staged_message_id)
            .await?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        let ids: Vec<String> =
            sqlx::query_scalar(r#"SELECT id FROM messages WHERE session_id = ? AND seq > ?"#)
                .bind(session_id.to_string())
                .bind(boundary_seq)
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        for id in ids {
            sqlx::query(r#"DELETE FROM message_parts WHERE message_id = ?"#)
                .bind(&id)
                .execute(&mut *tx)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
            sqlx::query(r#"DELETE FROM messages WHERE id = ?"#)
                .bind(&id)
                .execute(&mut *tx)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        }

        self.sessions
            .delete_pending_inputs_after_seq_tx(&mut tx, session_id, boundary_seq)
            .await?;

        sqlx::query(
            r#"DELETE FROM events
               WHERE session_id = ?
                 AND created_at > (
                     SELECT created_at FROM messages WHERE session_id = ? AND seq = ?
                 )"#,
        )
        .bind(session_id.to_string())
        .bind(session_id.to_string())
        .bind(boundary_seq)
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        // Drop the context epoch entirely: keeping the old baseline would keep
        // feeding summaries of the now-deleted messages to the model. The next
        // turn's ensure_epoch() re-renders a clean baseline.
        sqlx::query(r#"DELETE FROM context_epochs WHERE session_id = ?"#)
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        sqlx::query(r#"DELETE FROM session_revert WHERE session_id = ?"#)
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        self.events
            .append(
                EventKind::RevertCommitted,
                Some(session_id),
                json!({
                    "message_id": state.staged_message_id.to_string(),
                    "files": state.files.len(),
                    "warning": state.warning,
                }),
            )
            .await?;

        Ok(state)
    }

    async fn has_non_revertable_shell_after_seq(
        &self,
        session_id: SessionId,
        boundary_seq: i64,
    ) -> Result<bool> {
        let rows: Vec<String> = sqlx::query_scalar(
            r#"SELECT payload FROM events
               WHERE session_id = ?
                 AND kind = 'tool.completed'
                 AND created_at > (
                     SELECT created_at FROM messages WHERE session_id = ? AND seq = ?
                 )
               ORDER BY seq ASC"#,
        )
        .bind(session_id.to_string())
        .bind(session_id.to_string())
        .bind(boundary_seq)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        for payload in rows {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            let tool_name = value
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !matches!(tool_name, "bash" | "powershell") {
                continue;
            }
            let non_revertable = value
                .get("metadata")
                .and_then(|m| m.get("non_revertable"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if non_revertable {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn message_seq(&self, session_id: SessionId, message_id: MessageId) -> Result<i64> {
        let seq = sqlx::query_scalar::<_, i64>(
            r#"SELECT seq FROM messages WHERE session_id = ? AND id = ?"#,
        )
        .bind(session_id.to_string())
        .bind(message_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
        .ok_or_else(|| DeepSeekCodeError::NotFound(format!("message {message_id}")))?;
        Ok(seq)
    }
}

#[derive(sqlx::FromRow)]
struct RevertRow {
    staged_message_id: Option<String>,
    staged_at: Option<String>,
    diff_json: Option<String>,
}

impl RevertRow {
    fn into_state(self) -> Result<StagedRevert> {
        let message_id = self
            .staged_message_id
            .ok_or_else(|| DeepSeekCodeError::Storage("missing staged_message_id".into()))?;
        let staged_message_id = MessageId::from_str(&message_id)
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        let staged_at = self
            .staged_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let diff: serde_json::Value = self
            .diff_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        let files = diff
            .get("files")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(RevertFileEntry {
                            path: e.get("path")?.as_str()?.to_string(),
                            snapshot_id: e.get("snapshot_id")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let warning = diff
            .get("warning")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        Ok(StagedRevert {
            staged_message_id,
            staged_at,
            files,
            warning,
        })
    }
}
