use chrono::Utc;
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::session::{PromptDelivery, Session, SessionInput};
use deepseekcode_types::{DeepSeekCodeError, LocationId, MessageId, Result, SessionId};
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::paths::normalize_directory_path_for_lookup;

#[derive(Clone)]
pub struct SessionStore {
    pool: SqlitePool,
}

impl SessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find(&self, id: SessionId) -> Result<Option<Session>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.find",
            session_id = %id,
            "storage op start"
        );
        let row = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        let out = row.map(|r| r.into_session()).transpose();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.find",
            session_id = %id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            found = out.as_ref().map(|o| o.is_some()).unwrap_or(false),
            "storage op ok"
        );
        out
    }

    pub async fn create(
        &self,
        id: SessionId,
        location_id: LocationId,
        working_directory: &str,
        agent: AgentKind,
        model_id: Option<&str>,
        provider_id: Option<&str>,
    ) -> Result<Session> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.create",
            session_id = %id,
            "storage op start"
        );
        if let Some(existing) = self.find(id).await? {
            tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.create",
                session_id = %id,
                duration_ms = _op_start.elapsed().as_millis() as u64,
                "storage op ok (already exists)"
            );
            return Ok(existing);
        }

        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO sessions (id, location_id, title, agent, model_id, provider_id, working_directory, created_at, updated_at)
               VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(location_id.to_string())
        .bind(format!("{:?}", agent).to_lowercase())
        .bind(model_id)
        .bind(provider_id)
        .bind(working_directory)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.create",
            session_id = %id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(Session {
            id,
            location_id,
            title: None,
            agent,
            model_id: model_id.map(String::from),
            provider_id: provider_id.map(String::from),
            parent_session_id: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn get(&self, id: SessionId) -> Result<Session> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.get",
            session_id = %id,
            "storage op start"
        );
        let row = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?
        .ok_or_else(|| {
                tracing::warn!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session.get",
                    session_id = %id,
                    "session not found"
                );
                DeepSeekCodeError::NotFound(format!("session {id}"))
            })?;

        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.get",
            session_id = %id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        row.into_session()
    }

    pub async fn list(&self) -> Result<Vec<Session>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list",
            "storage op start"
        );
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions ORDER BY updated_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        let out: Result<Vec<Session>> = rows.into_iter().map(|r| r.into_session()).collect();
        match &out {
            Ok(sessions) => tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list",
                duration_ms = _op_start.elapsed().as_millis() as u64,
                rows = sessions.len(),
                "storage op ok"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list",
                error = %e,
                "storage op failed"
            ),
        }
        out
    }

    pub async fn list_for_working_directory(
        &self,
        working_directory: &str,
    ) -> Result<Vec<Session>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_for_working_directory",
            project_dir = %working_directory,
            "storage op start"
        );
        let key = normalize_working_directory(working_directory);
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at
               FROM sessions
               WHERE lower(rtrim(replace(working_directory, '\', '/'), '/')) = ?
               ORDER BY updated_at DESC"#,
        )
        .bind(&key)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        let out: Result<Vec<Session>> = rows.into_iter().map(|r| r.into_session()).collect();
        match &out {
            Ok(sessions) => tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list_for_working_directory",
                project_dir = %working_directory,
                duration_ms = _op_start.elapsed().as_millis() as u64,
                rows = sessions.len(),
                "storage op ok"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list_for_working_directory",
                error = %e,
                "storage op failed"
            ),
        }
        out
    }

    pub async fn list_child_sessions(&self, parent_id: SessionId) -> Result<Vec<SessionId>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_child_sessions",
            session_id = %parent_id,
            "storage op start"
        );
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT id FROM sessions WHERE parent_session_id = ? ORDER BY created_at ASC"#,
        )
        .bind(parent_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let out: Result<Vec<SessionId>> = rows
            .into_iter()
            .map(|(id,)| {
                SessionId::from_str(&id).map_err(|e| DeepSeekCodeError::Storage(e.to_string()))
            })
            .collect();
        match &out {
            Ok(ids) => tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list_child_sessions",
                session_id = %parent_id,
                duration_ms = _op_start.elapsed().as_millis() as u64,
                rows = ids.len(),
                "storage op ok"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.list_child_sessions",
                session_id = %parent_id,
                error = %e,
                "storage op failed"
            ),
        }
        out
    }

    pub async fn pending_inputs(&self, session_id: SessionId) -> Result<Vec<SessionInput>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.pending_inputs",
            session_id = %session_id,
            "storage op start"
        );
        // Order by pure admitted_seq when V11 exists, else legacy admitted_at.
        let has_seq = sqlx::query_scalar::<_, i64>(
            r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
        let rows = if has_seq {
            sqlx::query_as::<_, InputRow>(
                r#"SELECT id, session_id, prompt, delivery, admitted_at, promoted, admitted_seq FROM session_input WHERE session_id = ? AND promoted = 0 ORDER BY admitted_seq ASC, admitted_at ASC"#,
            )
            .bind(session_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?
        } else {
            sqlx::query_as::<_, InputRow>(
                r#"SELECT id, session_id, prompt, delivery, admitted_at, promoted FROM session_input WHERE session_id = ? AND promoted = 0 ORDER BY admitted_at ASC"#,
            )
            .bind(session_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?
        };

        let out = rows
            .into_iter()
            .map(|r| r.into_input())
            .collect::<Result<Vec<_>>>();
        match &out {
            Ok(inputs) => tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.pending_inputs",
                session_id = %session_id,
                duration_ms = _op_start.elapsed().as_millis() as u64,
                rows = inputs.len(),
                "storage op ok"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.pending_inputs",
                session_id = %session_id,
                error = %e,
                "storage op failed"
            ),
        }
        out
    }

    pub async fn promote_input(&self, message_id: MessageId) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.promote_input",
            message_id = %message_id,
            "storage op start"
        );
        let result = sqlx::query(r#"UPDATE session_input SET promoted = 1 WHERE id = ?"#)
            .bind(message_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.promote_input",
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            rows = result.rows_affected(),
            "storage op ok"
        );
        Ok(())
    }

    /// Atomically insert a user message and mark the matching inbox row promoted.
    pub async fn promote_input_atomic(
        &self,
        message_id: MessageId,
        session_id: SessionId,
        prompt: &str,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.promote_input_atomic",
            session_id = %session_id,
            message_id = %message_id,
            "storage op start"
        );
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'user', ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, 0, 'text', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(prompt)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        let updated = sqlx::query(
            r#"UPDATE session_input SET promoted = 1 WHERE id = ? AND session_id = ? AND promoted = 0"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        if updated.rows_affected() == 0 {
            tx.rollback().await.map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
            tracing::warn!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.promote_input_atomic",
                session_id = %session_id,
                message_id = %message_id,
                "inbox row missing or already promoted"
            );
            return Err(DeepSeekCodeError::Storage(
                "promote_input_atomic: inbox row missing or already promoted".into(),
            ));
        }
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.promote_input_atomic",
            session_id = %session_id,
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn has_unpromoted_pending(&self, session_id: SessionId) -> Result<bool> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.has_unpromoted_pending",
            session_id = %session_id,
            "storage op start"
        );
        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM session_input WHERE session_id = ? AND promoted = 0"#,
        )
        .bind(session_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.has_unpromoted_pending",
            session_id = %session_id,
            pending = count > 0,
            "storage op ok"
        );
        Ok(count > 0)
    }

    /// Delete unpromoted inbox entries admitted after the given message
    /// sequence, inside the caller's transaction (used by timeline revert).
    /// Pure-seq cut via admitted_seq (V11); wall-clock only for pre-V11 DBs.
    pub async fn delete_pending_inputs_after_seq_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: SessionId,
        boundary_seq: i64,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.delete_pending_inputs_after_seq_tx",
            session_id = %session_id,
            boundary_seq = boundary_seq,
            "storage op start"
        );
        // Resolve boundary message id → its inbox admitted_seq (the message
        // was promoted from inbox, so ids match). Pure integer comparison.
        let has_seq = sqlx::query_scalar::<_, i64>(
            r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
        )
        .fetch_one(&mut **tx)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
        if has_seq {
            let boundary_msg_id: Option<String> =
                sqlx::query_scalar(r#"SELECT id FROM messages WHERE session_id = ? AND seq = ?"#)
                    .bind(session_id.to_string())
                    .bind(boundary_seq)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            target: deepseekcode_telemetry::STORAGE,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            operation = "session_store",
                            error = %e,
                            "storage op failed"
                        );
                        DeepSeekCodeError::Storage(e.to_string())
                    })?;
            if let Some(mid) = boundary_msg_id {
                let cutoff: Option<i64> =
                    sqlx::query_scalar(r#"SELECT admitted_seq FROM session_input WHERE id = ?"#)
                        .bind(&mid)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(|e| {
                            tracing::error!(
                                target: deepseekcode_telemetry::STORAGE,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                operation = "session_store",
                                error = %e,
                                "storage op failed"
                            );
                            DeepSeekCodeError::Storage(e.to_string())
                        })?;
                if let Some(c) = cutoff {
                    sqlx::query(
                        r#"DELETE FROM session_input WHERE session_id = ? AND promoted = 0 AND admitted_seq > ?"#,
                    )
                    .bind(session_id.to_string())
                    .bind(c)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
                    tracing::info!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session.delete_pending_inputs_after_seq_tx",
                        session_id = %session_id,
                        "storage op ok"
                    );
                    return Ok(());
                }
            }
            // Boundary message has no inbox row (e.g. assistant/tool msg):
            // fall back to max admitted_seq among promoted rows up to boundary.
            // Conservative: delete unpromoted with seq greater than max
            // promoted seq for this session.
            let max_promoted: Option<i64> = sqlx::query_scalar(
                r#"SELECT MAX(admitted_seq) FROM session_input WHERE session_id = ? AND promoted = 1"#,
            )
            .bind(session_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?
            .flatten();
            if let Some(m) = max_promoted {
                sqlx::query(
                    r#"DELETE FROM session_input WHERE session_id = ? AND promoted = 0 AND admitted_seq > ?"#,
                )
                .bind(session_id.to_string())
                .bind(m)
                .execute(&mut **tx)
                .await
                .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
                tracing::info!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session.delete_pending_inputs_after_seq_tx",
                    session_id = %session_id,
                    "storage op ok"
                );
                return Ok(());
            }
            tracing::info!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session.delete_pending_inputs_after_seq_tx",
                session_id = %session_id,
                "storage op ok"
            );
            return Ok(());
        }
        sqlx::query(
            r#"DELETE FROM session_input
               WHERE session_id = ?
                 AND promoted = 0
                 AND admitted_at > (
                     SELECT created_at FROM messages WHERE session_id = ? AND seq = ?
                 )"#,
        )
        .bind(session_id.to_string())
        .bind(session_id.to_string())
        .bind(boundary_seq)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.delete_pending_inputs_after_seq_tx",
            session_id = %session_id,
            "storage op ok"
        );
        Ok(())
    }

    /// Latest assigned message sequence for a session (`-1` when empty).
    pub async fn latest_message_seq(&self, session_id: SessionId) -> Result<i64> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_message_seq",
            session_id = %session_id,
            "storage op start"
        );
        let seq: Option<i64> =
            sqlx::query_scalar(r#"SELECT MAX(seq) FROM messages WHERE session_id = ?"#)
                .bind(session_id.to_string())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session_store",
                        error = %e,
                        "storage op failed"
                    );
                    DeepSeekCodeError::Storage(e.to_string())
                })?;
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_message_seq",
            session_id = %session_id,
            seq = seq.unwrap_or(-1),
            "storage op ok"
        );
        Ok(seq.unwrap_or(-1))
    }

    pub async fn latest_message_seq_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: SessionId,
    ) -> Result<i64> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_message_seq_in_tx",
            session_id = %session_id,
            "storage op start"
        );
        let seq: Option<i64> =
            sqlx::query_scalar(r#"SELECT MAX(seq) FROM messages WHERE session_id = ?"#)
                .bind(session_id.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| {
                    tracing::error!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session_store",
                        error = %e,
                        "storage op failed"
                    );
                    DeepSeekCodeError::Storage(e.to_string())
                })?;
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_message_seq_in_tx",
            session_id = %session_id,
            seq = seq.unwrap_or(-1),
            "storage op ok"
        );
        Ok(seq.unwrap_or(-1))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: SessionId,
        location_id: LocationId,
        working_directory: &str,
        agent: AgentKind,
        model_id: Option<&str>,
        provider_id: Option<&str>,
    ) -> Result<Session> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.create_in_tx",
            session_id = %id,
            "storage op start"
        );
        let now = Utc::now();
        sqlx::query(
            r#"INSERT OR IGNORE INTO sessions (id, location_id, title, agent, model_id, provider_id, working_directory, created_at, updated_at)
               VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(location_id.to_string())
        .bind(format!("{:?}", agent).to_lowercase())
        .bind(model_id)
        .bind(provider_id)
        .bind(working_directory)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.create_in_tx",
            session_id = %id,
            "storage op ok"
        );
        Ok(Session {
            id,
            location_id,
            title: None,
            agent,
            model_id: model_id.map(String::from),
            provider_id: provider_id.map(String::from),
            parent_session_id: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn copy_unpromoted_inbox_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        from_session: SessionId,
        to_session: SessionId,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_unpromoted_inbox_in_tx",
            session_id = %to_session,
            "storage op start"
        );
        // Preserve admitted_seq when V11 exists; fall back when running on
        // pre-V11 DBs (column missing → plain copy).
        let has_seq = sqlx::query_scalar::<_, i64>(
            r#"SELECT COUNT(*) FROM pragma_table_info('session_input') WHERE name = 'admitted_seq'"#,
        )
        .fetch_one(&mut **tx)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
        if has_seq {
            sqlx::query(
                r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted, admitted_seq)
                   SELECT id, ?, prompt, delivery, admitted_at, promoted, admitted_seq FROM session_input
                   WHERE session_id = ? AND promoted = 0"#,
            )
            .bind(to_session.to_string())
            .bind(from_session.to_string())
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        } else {
            sqlx::query(
                r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted)
                   SELECT id, ?, prompt, delivery, admitted_at, promoted FROM session_input
                   WHERE session_id = ? AND promoted = 0"#,
            )
            .bind(to_session.to_string())
            .bind(from_session.to_string())
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        }
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_unpromoted_inbox_in_tx",
            session_id = %to_session,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn clear_pending_inputs(&self, session_id: SessionId) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.clear_pending_inputs",
            session_id = %session_id,
            "storage op start"
        );
        let result =
            sqlx::query(r#"DELETE FROM session_input WHERE session_id = ? AND promoted = 0"#)
                .bind(session_id.to_string())
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session_store",
                        error = %e,
                        "storage op failed"
                    );
                    DeepSeekCodeError::Storage(e.to_string())
                })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.clear_pending_inputs",
            session_id = %session_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            rows = result.rows_affected(),
            "storage op ok"
        );
        Ok(())
    }

    pub async fn update_session_model(
        &self,
        session_id: SessionId,
        agent: AgentKind,
        model_id: &str,
        provider_id: &str,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_session_model",
            session_id = %session_id,
            "storage op start"
        );
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE sessions SET agent = ?, model_id = ?, provider_id = ?, updated_at = ? WHERE id = ?"#,
        )
        .bind(format!("{:?}", agent).to_lowercase())
        .bind(model_id)
        .bind(provider_id)
        .bind(now)
        .bind(session_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_session_model",
            session_id = %session_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn update_title(&self, session_id: SessionId, title: &str) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_title",
            session_id = %session_id,
            "storage op start"
        );
        let now = Utc::now().to_rfc3339();
        sqlx::query(r#"UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?"#)
            .bind(title)
            .bind(now)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_title",
            session_id = %session_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn delete(&self, session_id: SessionId) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.delete",
            session_id = %session_id,
            "storage op start"
        );
        let id = session_id.to_string();
        // Collect on-disk sidecars BEFORE the tx so orphan files can be
        // removed after commit (DB rollback keeps rows; files stay).
        let snapshot_files: Vec<String> =
            sqlx::query_scalar(r#"SELECT snapshot_path FROM file_snapshots WHERE session_id = ?"#)
                .bind(&id)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        for sql in [
            r#"DELETE FROM message_parts WHERE message_id IN (SELECT id FROM messages WHERE session_id = ?)"#,
            r#"DELETE FROM messages WHERE session_id = ?"#,
            r#"DELETE FROM session_input WHERE session_id = ?"#,
            r#"DELETE FROM context_epochs WHERE session_id = ?"#,
            r#"DELETE FROM permissions WHERE session_id = ?"#,
            r#"DELETE FROM policy_grants WHERE id IN (SELECT id FROM policy_grants WHERE 0)"#,
            r#"DELETE FROM session_todos WHERE session_id = ?"#,
            r#"DELETE FROM session_plan_ui WHERE session_id = ?"#,
            r#"DELETE FROM session_plans WHERE session_id = ?"#,
            r#"DELETE FROM file_snapshots WHERE session_id = ?"#,
            r#"DELETE FROM session_revert WHERE session_id = ?"#,
            r#"DELETE FROM tool_results WHERE session_id = ?"#,
            r#"DELETE FROM events WHERE session_id = ?"#,
            r#"DELETE FROM sessions WHERE id = ?"#,
        ] {
            // Best-effort for tables that may not exist on old DBs (e.g.
            // session_plans/tool_results pre-migration): ignore "no such table".
            match sqlx::query(sql).bind(&id).execute(&mut *tx).await {
                Ok(_) => {}
                Err(e) if e.to_string().contains("no such table") => {}
                Err(e) => {
                    tx.rollback().await.ok();
                    return Err(DeepSeekCodeError::Storage(e.to_string()));
                }
            }
        }
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let snapshot_count = snapshot_files.len();
        for f in snapshot_files {
            let _ = std::fs::remove_file(&f);
        }
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.delete",
            session_id = %session_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            files = snapshot_count,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn append_text_part(
        &self,
        message_id: MessageId,
        part_index: i64,
        text: &str,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_text_part",
            message_id = %message_id,
            "storage op start"
        );
        let _op_start = std::time::Instant::now();
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'text', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(text)
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        self.touch_session_by_message(message_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_text_part",
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn append_tool_call(
        &self,
        message_id: MessageId,
        part_index: i64,
        tool_call_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_tool_call",
            message_id = %message_id,
            tool = %tool_name,
            "storage op start"
        );
        let _op_start = std::time::Instant::now();
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, tool_call_id, tool_name, tool_input)
               VALUES (?, ?, 'tool_call', ?, ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(tool_call_id)
        .bind(tool_name)
        .bind(input.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        self.touch_session_by_message(message_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_tool_call",
            message_id = %message_id,
            tool = %tool_name,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    async fn touch_session_by_message(&self, message_id: MessageId) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE sessions SET updated_at = ? WHERE id = (SELECT session_id FROM messages WHERE id = ?)"#,
        )
        .bind(now)
        .bind(message_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        Ok(())
    }

    async fn touch_session(&self, session_id: SessionId) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(r#"UPDATE sessions SET updated_at = ? WHERE id = ?"#)
            .bind(now)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        Ok(())
    }

    pub async fn insert_tool_call(
        &self,
        message_id: MessageId,
        session_id: SessionId,
        part_index: i64,
        tool_call_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_tool_call",
            session_id = %session_id,
            tool = %tool_name,
            "storage op start"
        );
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'assistant', ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, tool_call_id, tool_name, tool_input)
               VALUES (?, ?, 'tool_call', ?, ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(tool_call_id)
        .bind(tool_name)
        .bind(input.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        self.touch_session(session_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_tool_call",
            session_id = %session_id,
            tool = %tool_name,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn insert_tool_result(
        &self,
        message_id: MessageId,
        session_id: SessionId,
        part_index: i64,
        tool_call_id: &str,
        tool_name: &str,
        output: &serde_json::Value,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_tool_result",
            session_id = %session_id,
            tool = %tool_name,
            "storage op start"
        );
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'tool', ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, tool_call_id, tool_name, tool_output)
               VALUES (?, ?, 'tool_result', ?, ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(tool_call_id)
        .bind(tool_name)
        .bind(output.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        self.touch_session(session_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_tool_result",
            session_id = %session_id,
            tool = %tool_name,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn append_reasoning_part(
        &self,
        message_id: MessageId,
        part_index: i64,
        text: &str,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_reasoning_part",
            message_id = %message_id,
            "storage op start"
        );
        let _op_start = std::time::Instant::now();
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'reasoning', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(text)
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        self.touch_session_by_message(message_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.append_reasoning_part",
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn insert_assistant_turn(
        &self,
        id: MessageId,
        session_id: SessionId,
        reasoning: Option<&str>,
        text: &str,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_assistant_turn",
            session_id = %session_id,
            message_id = %id,
            "storage op start"
        );
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'assistant', ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        let mut part_index = 0i64;
        if let Some(reasoning) = reasoning.filter(|r| !r.is_empty()) {
            sqlx::query(
                r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'reasoning', ?)"#,
            )
            .bind(id.to_string())
            .bind(part_index)
            .bind(reasoning)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
            part_index += 1;
        }
        if !text.is_empty() {
            sqlx::query(
                r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'text', ?)"#,
            )
            .bind(id.to_string())
            .bind(part_index)
            .bind(text)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        }
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_assistant_turn",
            session_id = %session_id,
            message_id = %id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn insert_message(
        &self,
        id: MessageId,
        session_id: SessionId,
        role: &str,
        text: &str,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_message",
            session_id = %session_id,
            message_id = %id,
            "storage op start"
        );
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(session_id.to_string())
        .bind(role)
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, 0, 'text', ?)"#,
        )
        .bind(id.to_string())
        .bind(text)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_message",
            session_id = %session_id,
            message_id = %id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn list_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_messages",
            session_id = %session_id,
            "storage op start"
        );
        use deepseekcode_schema::message::{Message, MessagePart};

        let rows = sqlx::query_as::<_, MessageRow>(
            r#"SELECT id, role, seq, created_at FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC"#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        let mut messages = Vec::new();
        for row in rows {
            let parts = sqlx::query_as::<_, PartRow>(
                r#"SELECT kind, text, tool_call_id, tool_name, tool_input, tool_output
                   FROM message_parts WHERE message_id = ? ORDER BY part_index ASC"#,
            )
            .bind(&row.id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

            messages.push(Message {
                id: MessageId::from_str(&row.id).map_err(|e| {
                    tracing::error!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session_store",
                        error = %e,
                        "storage op failed"
                    );
                    DeepSeekCodeError::Storage(e.to_string())
                })?,
                session_id,
                role: parse_role(&row.role),
                seq: row.seq,
                parts: parts
                    .into_iter()
                    .map(|p| MessagePart {
                        kind: parse_part_kind(&p.kind),
                        text: p.text,
                        tool_call_id: p.tool_call_id.clone(),
                        tool_name: p.tool_name,
                        tool_input: p
                            .tool_input
                            .as_deref()
                            .and_then(|s| serde_json::from_str(s).ok()),
                        tool_output: p
                            .tool_output
                            .as_deref()
                            .and_then(|s| serde_json::from_str(s).ok()),
                    })
                    .collect(),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.created_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_messages",
            session_id = %session_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            rows = messages.len(),
            "storage op ok"
        );
        Ok(messages)
    }

    pub async fn list_messages_for_runner(
        &self,
        session_id: SessionId,
        baseline_seq: i64,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_messages_for_runner",
            session_id = %session_id,
            "storage op start"
        );
        let compaction_seq = self.latest_compaction_seq(session_id).await?;
        let messages = self.list_messages(session_id).await?;
        let out: Vec<deepseekcode_schema::message::Message> = messages
            .into_iter()
            .filter(|m| message_visible_to_runner(m, baseline_seq, compaction_seq))
            .collect();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.list_messages_for_runner",
            session_id = %session_id,
            rows = out.len(),
            "storage op ok"
        );
        Ok(out)
    }

    pub async fn latest_compaction_seq(&self, session_id: SessionId) -> Result<Option<i64>> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_compaction_seq",
            session_id = %session_id,
            "storage op start"
        );
        let row = sqlx::query_scalar::<_, i64>(
            r#"SELECT seq FROM messages WHERE session_id = ? AND role = 'compaction' ORDER BY seq DESC LIMIT 1"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_compaction_seq",
            session_id = %session_id,
            "storage op ok"
        );
        Ok(row)
    }

    #[allow(dead_code)]
    async fn next_message_seq(&self, session_id: SessionId) -> Result<i64> {
        let id = session_id.to_string();
        let seq = sqlx::query_scalar::<_, i64>(
            r#"UPDATE sessions SET next_message_seq = next_message_seq + 1 WHERE id = ? RETURNING next_message_seq - 1"#,
        )
        .bind(&id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        Ok(seq)
    }

    async fn next_message_seq_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: SessionId,
    ) -> Result<i64> {
        let id = session_id.to_string();
        let seq = sqlx::query_scalar::<_, i64>(
            r#"UPDATE sessions SET next_message_seq = next_message_seq + 1 WHERE id = ? RETURNING next_message_seq - 1"#,
        )
        .bind(&id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        Ok(seq)
    }

    pub async fn latest_compaction_cutoff(
        &self,
        session_id: SessionId,
    ) -> Result<Option<chrono::DateTime<Utc>>> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_compaction_cutoff",
            session_id = %session_id,
            "storage op start"
        );
        let row = sqlx::query_scalar::<_, String>(
            r#"SELECT created_at FROM messages WHERE session_id = ? AND role = 'compaction' ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        let out = row.and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(&ts)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        });
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.latest_compaction_cutoff",
            session_id = %session_id,
            found = out.is_some(),
            "storage op ok"
        );
        Ok(out)
    }

    async fn insert_copied_message_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        to_session: SessionId,
        msg: &deepseekcode_schema::message::Message,
    ) -> Result<()> {
        use deepseekcode_schema::message::PartKind;

        let now = Utc::now();
        let seq = self.next_message_seq_tx(tx, to_session).await?;
        let new_id = MessageId::new();

        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(new_id.to_string())
        .bind(to_session.to_string())
        .bind(format!("{:?}", msg.role).to_lowercase())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        for (idx, part) in msg.parts.iter().enumerate() {
            match part.kind {
                PartKind::Text | PartKind::Reasoning => {
                    sqlx::query(
                        r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, ?, ?)"#,
                    )
                    .bind(new_id.to_string())
                    .bind(idx as i64)
                    .bind(format!("{:?}", part.kind).to_lowercase())
                    .bind(part.text.as_deref())
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
                }
                PartKind::ToolCall => {
                    sqlx::query(
                        r#"INSERT INTO message_parts (message_id, part_index, kind, tool_call_id, tool_name, tool_input)
                           VALUES (?, ?, 'tool_call', ?, ?, ?)"#,
                    )
                    .bind(new_id.to_string())
                    .bind(idx as i64)
                    .bind(part.tool_call_id.as_deref())
                    .bind(part.tool_name.as_deref())
                    .bind(part.tool_input.as_ref().map(|v| v.to_string()))
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
                }
                PartKind::ToolResult => {
                    sqlx::query(
                        r#"INSERT INTO message_parts (message_id, part_index, kind, tool_call_id, tool_name, tool_output)
                           VALUES (?, ?, 'tool_result', ?, ?, ?)"#,
                    )
                    .bind(new_id.to_string())
                    .bind(idx as i64)
                    .bind(part.tool_call_id.as_deref())
                    .bind(part.tool_name.as_deref())
                    .bind(part.tool_output.as_ref().map(|v| v.to_string()))
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
                }
            }
        }
        Ok(())
    }

    pub async fn copy_messages_with_parts_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        from_messages: &[deepseekcode_schema::message::Message],
        to_session: SessionId,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_with_parts_in_tx",
            session_id = %to_session,
            "storage op start"
        );
        for msg in from_messages {
            self.insert_copied_message_in_tx(tx, to_session, msg)
                .await?;
        }
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_with_parts_in_tx",
            session_id = %to_session,
            rows = from_messages.len(),
            "storage op ok"
        );
        Ok(())
    }

    pub async fn copy_messages_with_parts(
        &self,
        from_session: SessionId,
        to_session: SessionId,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_with_parts",
            session_id = %to_session,
            "storage op start"
        );
        let messages = self.list_messages(from_session).await?;
        // Atomic: single transaction so concurrent forks/copies never interleave
        // seqs and a mid-copy failure never leaves a half-copied session.
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        self.copy_messages_with_parts_in_tx(&mut tx, &messages, to_session)
            .await?;
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_with_parts",
            session_id = %to_session,
            rows = messages.len(),
            "storage op ok"
        );
        Ok(())
    }

    pub async fn update_message_tokens(
        &self,
        message_id: MessageId,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_message_tokens",
            message_id = %message_id,
            "storage op start"
        );
        self.update_message_tokens_with_cache(
            message_id,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            None,
        )
        .await
    }

    pub async fn update_message_tokens_with_cache(
        &self,
        message_id: MessageId,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
        cache_tokens: Option<i64>,
    ) -> Result<()> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_message_tokens_with_cache",
            message_id = %message_id,
            "storage op start"
        );
        sqlx::query(
            r#"UPDATE messages SET input_tokens = ?, output_tokens = ?, reasoning_tokens = ?, cache_tokens = COALESCE(?, cache_tokens) WHERE id = ?"#,
        )
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(reasoning_tokens)
        .bind(cache_tokens)
        .bind(message_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        self.touch_session_by_message(message_id).await?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.update_message_tokens_with_cache",
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn copy_messages_until_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        from_messages: &[deepseekcode_schema::message::Message],
        to_session: SessionId,
        until_message_id: MessageId,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_until_in_tx",
            session_id = %to_session,
            "storage op start"
        );
        let cutoff_seq = from_messages
            .iter()
            .find(|m| m.id == until_message_id)
            .map(|m| m.seq)
            .ok_or_else(|| {
                tracing::warn!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session.copy_messages_until_in_tx",
                    session_id = %to_session,
                    message_id = %until_message_id,
                    "message not found in session"
                );
                DeepSeekCodeError::NotFound(format!("message {until_message_id} not in session"))
            })?;
        for msg in from_messages.iter().filter(|m| m.seq <= cutoff_seq) {
            self.insert_copied_message_in_tx(tx, to_session, msg)
                .await?;
        }
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_until_in_tx",
            session_id = %to_session,
            "storage op ok"
        );
        Ok(())
    }

    pub async fn copy_messages_until(
        &self,
        from_session: SessionId,
        to_session: SessionId,
        until_message_id: MessageId,
    ) -> Result<()> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_until",
            session_id = %to_session,
            "storage op start"
        );
        let messages = self.list_messages(from_session).await?;
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        self.copy_messages_until_in_tx(&mut tx, &messages, to_session, until_message_id)
            .await?;
        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.copy_messages_until",
            session_id = %to_session,
            rows = messages.len(),
            "storage op ok"
        );
        Ok(())
    }

    // NOTE: a `delete_messages_after_seq` helper lived here. It was removed:
    // zero callers, and row deletion would violate the canonical-transcript
    // invariant (history is append-only; rewind/fork/visibility are handled
    // by seq filtering, never DELETE). Re-add only with a design review.

    pub async fn insert_compaction(
        &self,
        session_id: SessionId,
        message_id: MessageId,
        summary: &str,
        recent: &str,
    ) -> Result<i64> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.insert_compaction",
            session_id = %session_id,
            "storage op start"
        );
        self.commit_compaction_with_epoch(session_id, message_id, summary, recent, None, None)
            .await
    }

    /// Atomically insert a compaction checkpoint and optionally replace the context epoch baseline.
    /// Issue-13: `lineage_json` carries `{lineage,tools_hash,model,covered_hash,version}`
    /// so later turns can validate the checkpoint belongs to the current
    /// prefix lineage instead of blindly reusing a stale summary.
    pub async fn commit_compaction_with_epoch(
        &self,
        session_id: SessionId,
        message_id: MessageId,
        summary: &str,
        recent: &str,
        baseline: Option<&str>,
        snapshot_json: Option<&str>,
    ) -> Result<i64> {
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.commit_compaction_with_epoch",
            session_id = %session_id,
            "storage op start"
        );
        self.commit_compaction_with_lineage(
            session_id,
            message_id,
            summary,
            recent,
            baseline,
            snapshot_json,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_compaction_with_lineage(
        &self,
        session_id: SessionId,
        message_id: MessageId,
        summary: &str,
        recent: &str,
        baseline: Option<&str>,
        snapshot_json: Option<&str>,
        lineage_json: Option<&serde_json::Value>,
    ) -> Result<i64> {
        let _op_start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.commit_compaction_with_lineage",
            session_id = %session_id,
            "storage op start"
        );
        let mut tx = self.pool.begin().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;

        let now = Utc::now();
        let seq = self.next_message_seq_tx(&mut tx, session_id).await?;
        let mut payload = serde_json::json!({ "summary": summary, "recent": recent, "version": 1 });
        if let Some(extra) = lineage_json {
            if let (Some(obj), Some(ex)) = (payload.as_object_mut(), extra.as_object()) {
                for (k, v) in ex {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'compaction', ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, 0, 'text', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(payload.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;

        if let (Some(baseline), Some(snapshot_json)) = (baseline, snapshot_json) {
            sqlx::query(
                r#"INSERT OR REPLACE INTO context_epochs (session_id, baseline, snapshot, baseline_seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
            )
            .bind(session_id.to_string())
            .bind(baseline)
            .bind(snapshot_json)
            .bind(seq)
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?;
        }

        tx.commit().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "session_store",
                error = %e,
                "storage op failed"
            );
            DeepSeekCodeError::Storage(e.to_string())
        })?;
        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "session.commit_compaction_with_lineage",
            session_id = %session_id,
            message_id = %message_id,
            duration_ms = _op_start.elapsed().as_millis() as u64,
            seq = seq,
            "storage op ok"
        );
        Ok(seq)
    }
}

pub fn message_visible_to_runner(
    message: &deepseekcode_schema::message::Message,
    baseline_seq: i64,
    compaction_seq: Option<i64>,
) -> bool {
    use deepseekcode_schema::message::MessageRole;
    let passes_baseline = message.role != MessageRole::System || message.seq > baseline_seq;
    if !passes_baseline {
        return false;
    }
    match compaction_seq {
        None => true,
        Some(cutoff) => {
            message.seq >= cutoff
                || (message.role == MessageRole::System && message.seq > baseline_seq)
        }
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: String,
    role: String,
    seq: i64,
    created_at: String,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    location_id: String,
    title: Option<String>,
    agent: String,
    model_id: Option<String>,
    provider_id: Option<String>,
    parent_session_id: Option<String>,
    created_at: String,
    updated_at: String,
}

impl SessionRow {
    fn into_session(self) -> Result<Session> {
        Ok(Session {
            id: SessionId::from_str(&self.id).map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?,
            location_id: LocationId::from_str(&self.location_id).map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::STORAGE,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    operation = "session_store",
                    error = %e,
                    "storage op failed"
                );
                DeepSeekCodeError::Storage(e.to_string())
            })?,
            title: self.title,
            agent: parse_agent(&self.agent),
            model_id: self.model_id,
            provider_id: self.provider_id,
            parent_session_id: self
                .parent_session_id
                .as_deref()
                .map(SessionId::from_str)
                .transpose()
                .map_err(|e| {
                    tracing::error!(
                        target: deepseekcode_telemetry::STORAGE,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        operation = "session_store",
                        error = %e,
                        "storage op failed"
                    );
                    DeepSeekCodeError::Storage(e.to_string())
                })?,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&self.updated_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

#[derive(sqlx::FromRow)]
struct InputRow {
    id: String,
    session_id: String,
    prompt: String,
    delivery: String,
    admitted_at: String,
    promoted: i64,
    #[sqlx(default)]
    #[allow(dead_code)]
    admitted_seq: Option<i64>,
}

impl InputRow {
    fn into_input(self) -> Result<SessionInput> {
        Ok(SessionInput {
            id: MessageId::from_str(&self.id).map_err(|_| {
                DeepSeekCodeError::Storage(format!(
                    "invalid message id in session_input: {}",
                    self.id
                ))
            })?,
            session_id: SessionId::from_str(&self.session_id).map_err(|_| {
                DeepSeekCodeError::Storage(format!(
                    "invalid session id in session_input: {}",
                    self.session_id
                ))
            })?,
            prompt: self.prompt,
            delivery: if self.delivery == "steer" {
                PromptDelivery::Steer
            } else {
                PromptDelivery::Queue
            },
            admitted_at: chrono::DateTime::parse_from_rfc3339(&self.admitted_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            promoted: self.promoted != 0,
        })
    }
}

fn normalize_working_directory(path: &str) -> String {
    normalize_directory_path_for_lookup(path)
}

fn parse_agent(s: &str) -> AgentKind {
    match s {
        "plan" => AgentKind::Plan,
        "general" => AgentKind::General,
        _ => AgentKind::Build,
    }
}

#[derive(sqlx::FromRow)]
struct PartRow {
    kind: String,
    text: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_output: Option<String>,
}

fn parse_role(s: &str) -> deepseekcode_schema::message::MessageRole {
    match s {
        "assistant" => deepseekcode_schema::message::MessageRole::Assistant,
        "system" => deepseekcode_schema::message::MessageRole::System,
        "tool" => deepseekcode_schema::message::MessageRole::Tool,
        "compaction" => deepseekcode_schema::message::MessageRole::Compaction,
        _ => deepseekcode_schema::message::MessageRole::User,
    }
}

fn parse_part_kind(s: &str) -> deepseekcode_schema::message::PartKind {
    match s {
        "reasoning" => deepseekcode_schema::message::PartKind::Reasoning,
        "tool_call" => deepseekcode_schema::message::PartKind::ToolCall,
        "tool_result" => deepseekcode_schema::message::PartKind::ToolResult,
        _ => deepseekcode_schema::message::PartKind::Text,
    }
}
