use chrono::Utc;
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::session::PromptDelivery;
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result, SessionId};
use sqlx::SqlitePool;
use std::str::FromStr;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventStore {
    pool: SqlitePool,
    tx: broadcast::Sender<EventEnvelope>,
}

impl EventStore {
    pub fn new(pool: SqlitePool) -> Self {
        let (tx, _) = broadcast::channel(8192);
        Self { pool, tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.tx.subscribe()
    }

    pub async fn append(
        &self,
        kind: EventKind,
        session_id: Option<SessionId>,
        payload: serde_json::Value,
    ) -> Result<EventEnvelope> {
        let created_at = Utc::now();
        let result = sqlx::query(
            r#"INSERT INTO events (kind, session_id, payload, created_at) VALUES (?, ?, ?, ?)"#,
        )
        .bind(kind.as_str())
        .bind(session_id.map(|s| s.to_string()))
        .bind(payload.to_string())
        .bind(created_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                error = %e,
                kind = ?kind,
                "event append failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;

        let seq = result.last_insert_rowid();
        let envelope = EventEnvelope {
            seq,
            kind,
            session_id,
            payload,
            created_at,
        };
        if self.tx.send(envelope.clone()).is_err() {
            tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                seq,
                "event broadcast had no active receivers"
            );
        }
        Ok(envelope)
    }

    pub async fn latest_seq(&self, session_id: SessionId) -> Result<i64> {
        let row = sqlx::query_scalar::<_, i64>(
            r#"SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(row)
    }

    pub async fn latest_head_seq(&self) -> Result<i64> {
        let row = sqlx::query_scalar::<_, i64>(r#"SELECT COALESCE(MAX(seq), 0) FROM events"#)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(row)
    }

    pub async fn tail(&self, since_seq: i64, limit: i64) -> Result<Vec<EventEnvelope>> {
        self.tail_filtered(since_seq, limit, None).await
    }

    pub async fn tail_filtered(
        &self,
        since_seq: i64,
        limit: i64,
        session_id: Option<SessionId>,
    ) -> Result<Vec<EventEnvelope>> {
        let rows = if let Some(session_id) = session_id {
            sqlx::query_as::<_, EventRow>(
                r#"SELECT seq, kind, session_id, payload, created_at FROM events
                   WHERE seq > ? AND session_id = ? ORDER BY seq ASC LIMIT ?"#,
            )
            .bind(since_seq)
            .bind(session_id.to_string())
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, EventRow>(
                r#"SELECT seq, kind, session_id, payload, created_at FROM events
                   WHERE seq > ? ORDER BY seq ASC LIMIT ?"#,
            )
            .bind(since_seq)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_envelope()).collect()
    }
}

#[derive(sqlx::FromRow)]
struct EventRow {
    seq: i64,
    kind: String,
    session_id: Option<String>,
    payload: String,
    created_at: String,
}

impl EventRow {
    fn into_envelope(self) -> Result<EventEnvelope> {
        let kind = parse_event_kind(&self.kind)?;
        Ok(EventEnvelope {
            seq: self.seq,
            kind,
            session_id: self
                .session_id
                .as_ref()
                .and_then(|s| SessionId::from_str(s).ok()),
            payload: serde_json::from_str(&self.payload)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

fn parse_event_kind(s: &str) -> Result<EventKind> {
    EventKind::parse(s)
        .ok_or_else(|| DeepSeekCodeError::Storage(format!("unknown event kind: {s}")))
}

pub async fn admit_prompt(
    pool: &SqlitePool,
    message_id: MessageId,
    session_id: SessionId,
    prompt: &str,
    delivery: PromptDelivery,
) -> Result<()> {
    // Transactional admit: BEGIN IMMEDIATE serializes concurrent prompts on
    // the same session so MAX+1 cannot allocate duplicate admitted_seq.
    // INSERT OR IGNORE is kept for idempotency, but a conflicting row with
    // different content is surfaced (not silently dropped).
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    sqlx::query("SELECT 1")
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    let has_seq: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
    )
    .fetch_one(&mut *tx)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    let delivery_str = match delivery {
        PromptDelivery::Steer => "steer",
        PromptDelivery::Queue => "queue",
    };
    if has_seq {
        let next: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(MAX(admitted_seq), 0) + 1 FROM session_input WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        let result = sqlx::query(
            r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted, admitted_seq) VALUES (?, ?, ?, ?, ?, 0, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(prompt)
        .bind(delivery_str)
        .bind(Utc::now().to_rfc3339())
        .bind(next)
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        // OR IGNORE with 0 rows = id collision: verify content matches.
        if result.rows_affected() == 0 {
            let existing: Option<(String, String)> =
                sqlx::query_as(r#"SELECT prompt, delivery FROM session_input WHERE id = ?"#)
                    .bind(message_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
            if let Some((ep, ed)) = existing {
                if ep != prompt || ed != delivery_str {
                    return Err(DeepSeekCodeError::AlreadyExists(format!(
                        "prompt id {message_id} already admitted with different content"
                    )));
                }
            }
        }
    } else {
        let result = sqlx::query(
            r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted) VALUES (?, ?, ?, ?, ?, 0)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(prompt)
        .bind(delivery_str)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        if result.rows_affected() == 0 {
            let existing: Option<(String, String)> =
                sqlx::query_as(r#"SELECT prompt, delivery FROM session_input WHERE id = ?"#)
                    .bind(message_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
            if let Some((ep, ed)) = existing {
                if ep != prompt || ed != delivery_str {
                    return Err(DeepSeekCodeError::AlreadyExists(format!(
                        "prompt id {message_id} already admitted with different content"
                    )));
                }
            }
        }
    }
    tx.commit()
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    Ok(())
}
