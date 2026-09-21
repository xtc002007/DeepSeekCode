use super::source::{ContextKey, Generation, ReconcileResult, Snapshot, SourceSnapshot};
use deepseekcode_types::{DeepSeekCodeError, Result, SessionId};
use sqlx::SqlitePool;

pub struct ContextEpoch {
    pub session_id: SessionId,
    pub baseline: String,
    pub snapshot: Snapshot,
    pub baseline_seq: i64,
}

pub struct ContextEpochStore {
    pool: SqlitePool,
}

impl ContextEpochStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, session_id: SessionId) -> Result<Option<ContextEpoch>> {
        let row = sqlx::query_as::<_, EpochRow>(
            r#"SELECT baseline, snapshot, baseline_seq FROM context_epochs WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        Ok(row.map(|r| ContextEpoch {
            session_id,
            baseline: r.baseline,
            snapshot: serde_json::from_str(&r.snapshot).unwrap_or_default(),
            baseline_seq: r.baseline_seq,
        }))
    }

    pub async fn initialize(
        &self,
        session_id: SessionId,
        generation: &Generation,
        baseline_seq: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT OR REPLACE INTO context_epochs (session_id, baseline, snapshot, baseline_seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(session_id.to_string())
        .bind(&generation.baseline)
        .bind(serde_json::to_string(&generation.snapshot)?)
        .bind(baseline_seq)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn update_snapshot(&self, session_id: SessionId, snapshot: &Snapshot) -> Result<()> {
        sqlx::query(r#"UPDATE context_epochs SET snapshot = ? WHERE session_id = ?"#)
            .bind(serde_json::to_string(snapshot)?)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn replace_baseline(
        &self,
        session_id: SessionId,
        generation: &Generation,
        baseline_seq: i64,
    ) -> Result<()> {
        self.initialize(session_id, generation, baseline_seq).await
    }

    pub async fn clear(&self, session_id: SessionId) -> Result<()> {
        sqlx::query(r#"DELETE FROM context_epochs WHERE session_id = ?"#)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn update_baseline_seq(
        &self,
        session_id: SessionId,
        baseline_seq: i64,
    ) -> Result<()> {
        sqlx::query(r#"UPDATE context_epochs SET baseline_seq = ? WHERE session_id = ?"#)
            .bind(baseline_seq)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn copy_epoch(
        &self,
        from_session: SessionId,
        to_session: SessionId,
        baseline_seq: i64,
    ) -> Result<()> {
        let row = sqlx::query_as::<_, EpochRow>(
            r#"SELECT baseline, snapshot, baseline_seq FROM context_epochs WHERE session_id = ?"#,
        )
        .bind(from_session.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        let Some(row) = row else {
            return Ok(());
        };
        sqlx::query(
            r#"INSERT OR REPLACE INTO context_epochs (session_id, baseline, snapshot, baseline_seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(to_session.to_string())
        .bind(row.baseline)
        .bind(row.snapshot)
        .bind(baseline_seq)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct EpochRow {
    baseline: String,
    snapshot: String,
    baseline_seq: i64,
}

pub fn reconcile_snapshot_removed(
    previous: &Snapshot,
    key: &ContextKey,
    removed_text: String,
) -> ReconcileResult {
    let key_str = key.0.clone();
    if !previous.contains_key(&key_str) {
        return ReconcileResult::Unchanged;
    }

    let mut snapshot = previous.clone();
    snapshot.remove(&key_str);

    ReconcileResult::Updated {
        text: removed_text,
        snapshot,
    }
}

pub fn reconcile_snapshot(
    previous: &Snapshot,
    key: &ContextKey,
    encoded: serde_json::Value,
    update_text: String,
) -> ReconcileResult {
    let key_str = key.0.clone();
    let prev_value = previous.get(&key_str).map(|s| s.value.clone());

    if prev_value.as_ref() == Some(&encoded) {
        return ReconcileResult::Unchanged;
    }

    let mut snapshot = previous.clone();
    snapshot.insert(
        key_str,
        SourceSnapshot {
            value: encoded,
            removed: None,
        },
    );

    ReconcileResult::Updated {
        text: update_text,
        snapshot,
    }
}
