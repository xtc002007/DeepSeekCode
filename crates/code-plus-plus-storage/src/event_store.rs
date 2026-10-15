use chrono::Utc;
use code_plus_plus_schema::event::{EventEnvelope, EventKind};
use code_plus_plus_schema::session::PromptDelivery;
use code_plus_plus_types::{CodePlusPlusError, MessageId, Result, SessionId};
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
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                error = %e,
                kind = ?kind,
                "event append failed"
            );
            CodePlusPlusError::Storage(e.to_string())
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
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
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
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;
        Ok(row)
    }

    pub async fn latest_head_seq(&self) -> Result<i64> {
        let row = sqlx::query_scalar::<_, i64>(r#"SELECT COALESCE(MAX(seq), 0) FROM events"#)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;
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
        .map_err(|e| CodePlusPlusError::Storage(e.to_string()))?;

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
        .ok_or_else(|| CodePlusPlusError::Storage(format!("unknown event kind: {s}")))
}

pub async fn admit_prompt(
    pool: &SqlitePool,
    message_id: MessageId,
    session_id: SessionId,
    prompt: &str,
    delivery: PromptDelivery,
) -> Result<()> {
    // True `BEGIN IMMEDIATE` (not `pool.begin()`/DEFERRED) serializes
    // concurrent prompts on the same session so MAX+1 cannot allocate
    // duplicate admitted_seq. `acquire_immediate` takes the RESERVED lock up
    // front with busy retry; raw BEGIN has no Drop guard so every error path
    // rolls back explicitly. `has_seq` is probed per call (no process-global
    // cache) so parallel tests on different DBs never cross-pollute.
    let mut conn = crate::database::acquire_immediate(pool).await?;
    let has_seq: bool = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
    )
    .fetch_one(&mut *conn)
    .await
    .map(|c: i64| c > 0)
    .unwrap_or(false);
    let delivery_str = match delivery {
        PromptDelivery::Steer => "steer",
        PromptDelivery::Queue => "queue",
    };
    // Local helper: rollback the raw IMMEDIATE tx, then map to storage error.
    macro_rules! fail {
        ($e:expr) => {{
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(CodePlusPlusError::Storage($e.to_string()));
        }};
    }
    if has_seq {
        let next: i64 = match sqlx::query_scalar(
            r#"SELECT COALESCE(MAX(admitted_seq), 0) + 1 FROM session_input WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_one(&mut *conn)
        .await
        {
            Ok(v) => v,
            Err(e) => fail!(e),
        };
        let result = match sqlx::query(
            r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted, admitted_seq) VALUES (?, ?, ?, ?, ?, 0, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(prompt)
        .bind(delivery_str)
        .bind(Utc::now().to_rfc3339())
        .bind(next)
        .execute(&mut *conn)
        .await
        {
            Ok(v) => v,
            Err(e) => fail!(e),
        };
        // OR IGNORE with 0 rows = id collision: verify content matches.
        if result.rows_affected() == 0 {
            let existing: Option<(String, String)> = match sqlx::query_as(
                r#"SELECT prompt, delivery FROM session_input WHERE id = ?"#,
            )
            .bind(message_id.to_string())
            .fetch_optional(&mut *conn)
            .await
            {
                Ok(v) => v,
                Err(e) => fail!(e),
            };
            if let Some((ep, ed)) = existing {
                if ep != prompt || ed != delivery_str {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(CodePlusPlusError::AlreadyExists(format!(
                        "prompt id {message_id} already admitted with different content"
                    )));
                }
            }
        }
    } else {
        let result = match sqlx::query(
            r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted) VALUES (?, ?, ?, ?, ?, 0)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(prompt)
        .bind(delivery_str)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *conn)
        .await
        {
            Ok(v) => v,
            Err(e) => fail!(e),
        };
        if result.rows_affected() == 0 {
            let existing: Option<(String, String)> = match sqlx::query_as(
                r#"SELECT prompt, delivery FROM session_input WHERE id = ?"#,
            )
            .bind(message_id.to_string())
            .fetch_optional(&mut *conn)
            .await
            {
                Ok(v) => v,
                Err(e) => fail!(e),
            };
            if let Some((ep, ed)) = existing {
                if ep != prompt || ed != delivery_str {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(CodePlusPlusError::AlreadyExists(format!(
                        "prompt id {message_id} already admitted with different content"
                    )));
                }
            }
        }
    }
    if let Err(e) = sqlx::query("COMMIT").execute(&mut *conn).await {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        return Err(CodePlusPlusError::Storage(e.to_string()));
    }
    Ok(())
}
