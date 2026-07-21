use chrono::Utc;
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::session::{PromptDelivery, Session, SessionInput};
use deepseekcode_types::{DeepSeekCodeError, LocationId, MessageId, Result, SessionId};
use sqlx::SqlitePool;
use std::str::FromStr;

pub struct SessionStore {
    pool: SqlitePool,
}

impl SessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find(&self, id: SessionId) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        row.map(|r| r.into_session()).transpose()
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
        if let Some(existing) = self.find(id).await? {
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
        let row = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?
        .ok_or_else(|| DeepSeekCodeError::NotFound(format!("session {id}")))?;

        row.into_session()
    }

    pub async fn list(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"SELECT id, location_id, title, agent, model_id, provider_id, parent_session_id, created_at, updated_at FROM sessions ORDER BY updated_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_session()).collect()
    }

    pub async fn list_for_working_directory(
        &self,
        working_directory: &str,
    ) -> Result<Vec<Session>> {
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter().map(|r| r.into_session()).collect()
    }

    pub async fn list_child_sessions(&self, parent_id: SessionId) -> Result<Vec<SessionId>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT id FROM sessions WHERE parent_session_id = ? ORDER BY created_at ASC"#,
        )
        .bind(parent_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        rows.into_iter()
            .map(|(id,)| {
                SessionId::from_str(&id).map_err(|e| DeepSeekCodeError::Storage(e.to_string()))
            })
            .collect()
    }

    pub async fn pending_inputs(&self, session_id: SessionId) -> Result<Vec<SessionInput>> {
        let rows = sqlx::query_as::<_, InputRow>(
            r#"SELECT id, session_id, prompt, delivery, admitted_at, promoted FROM session_input WHERE session_id = ? AND promoted = 0 ORDER BY admitted_at ASC"#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        rows.into_iter()
            .map(|r| r.into_input())
            .collect::<Result<Vec<_>>>()
    }

    pub async fn promote_input(&self, message_id: MessageId) -> Result<()> {
        sqlx::query(r#"UPDATE session_input SET promoted = 1 WHERE id = ?"#)
            .bind(message_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn clear_pending_inputs(&self, session_id: SessionId) -> Result<()> {
        sqlx::query(r#"DELETE FROM session_input WHERE session_id = ? AND promoted = 0"#)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn update_session_model(
        &self,
        session_id: SessionId,
        agent: AgentKind,
        model_id: &str,
        provider_id: &str,
    ) -> Result<()> {
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn update_title(&self, session_id: SessionId, title: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(r#"UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?"#)
            .bind(title)
            .bind(now)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn delete(&self, session_id: SessionId) -> Result<()> {
        let id = session_id.to_string();
        sqlx::query(
            r#"DELETE FROM message_parts WHERE message_id IN (SELECT id FROM messages WHERE session_id = ?)"#,
        )
        .bind(&id)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM messages WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM session_input WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM context_epochs WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM permissions WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM session_todos WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM session_plan_ui WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM file_snapshots WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM session_revert WHERE session_id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query(r#"DELETE FROM sessions WHERE id = ?"#)
            .bind(&id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn append_text_part(
        &self,
        message_id: MessageId,
        part_index: i64,
        text: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'text', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(text)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn append_reasoning_part(
        &self,
        message_id: MessageId,
        part_index: i64,
        text: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, ?, 'reasoning', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(part_index)
        .bind(text)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn insert_assistant_turn(
        &self,
        id: MessageId,
        session_id: SessionId,
        reasoning: Option<&str>,
        text: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        }
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn insert_message(
        &self,
        id: MessageId,
        session_id: SessionId,
        role: &str,
        text: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, 0, 'text', ?)"#,
        )
        .bind(id.to_string())
        .bind(text)
        .execute(&mut *tx)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn list_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        use deepseekcode_schema::message::{Message, MessagePart};

        let rows = sqlx::query_as::<_, MessageRow>(
            r#"SELECT id, role, seq, created_at FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC"#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        let mut messages = Vec::new();
        for row in rows {
            let parts = sqlx::query_as::<_, PartRow>(
                r#"SELECT kind, text, tool_call_id, tool_name, tool_input, tool_output
                   FROM message_parts WHERE message_id = ? ORDER BY part_index ASC"#,
            )
            .bind(&row.id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

            messages.push(Message {
                id: MessageId::from_str(&row.id)
                    .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
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
        Ok(messages)
    }

    pub async fn list_messages_for_runner(
        &self,
        session_id: SessionId,
        baseline_seq: i64,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        let compaction_seq = self.latest_compaction_seq(session_id).await?;
        let messages = self.list_messages(session_id).await?;
        Ok(messages
            .into_iter()
            .filter(|m| message_visible_to_runner(m, baseline_seq, compaction_seq))
            .collect())
    }

    pub async fn latest_compaction_seq(&self, session_id: SessionId) -> Result<Option<i64>> {
        let row = sqlx::query_scalar::<_, i64>(
            r#"SELECT seq FROM messages WHERE session_id = ? AND role = 'compaction' ORDER BY seq DESC LIMIT 1"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(row)
    }

    async fn next_message_seq(&self, session_id: SessionId) -> Result<i64> {
        let id = session_id.to_string();
        let seq = sqlx::query_scalar::<_, i64>(
            r#"UPDATE sessions SET next_message_seq = next_message_seq + 1 WHERE id = ? RETURNING next_message_seq - 1"#,
        )
        .bind(&id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(seq)
    }

    pub async fn latest_compaction_cutoff(
        &self,
        session_id: SessionId,
    ) -> Result<Option<chrono::DateTime<Utc>>> {
        let row = sqlx::query_scalar::<_, String>(
            r#"SELECT created_at FROM messages WHERE session_id = ? AND role = 'compaction' ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(row.and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(&ts)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }))
    }

    pub async fn copy_messages_with_parts(
        &self,
        from_session: SessionId,
        to_session: SessionId,
    ) -> Result<()> {
        use deepseekcode_schema::message::PartKind;

        let messages = self.list_messages(from_session).await?;
        for msg in messages {
            let now = Utc::now();
            let seq = self.next_message_seq(to_session).await?;
            let new_id = MessageId::new();

            sqlx::query(
                r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
            )
            .bind(new_id.to_string())
            .bind(to_session.to_string())
            .bind(format!("{:?}", msg.role).to_lowercase())
            .bind(seq)
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn update_message_tokens(
        &self,
        message_id: MessageId,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE messages SET input_tokens = ?, output_tokens = ?, reasoning_tokens = ? WHERE id = ?"#,
        )
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(reasoning_tokens)
        .bind(message_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn copy_messages_until(
        &self,
        from_session: SessionId,
        to_session: SessionId,
        until_message_id: MessageId,
    ) -> Result<()> {
        use deepseekcode_schema::message::PartKind;

        let messages = self.list_messages(from_session).await?;
        let cutoff_seq = messages
            .iter()
            .find(|m| m.id == until_message_id)
            .map(|m| m.seq)
            .ok_or_else(|| {
                DeepSeekCodeError::NotFound(format!("message {until_message_id} not in session"))
            })?;

        for msg in messages.into_iter().filter(|m| m.seq <= cutoff_seq) {
            let now = Utc::now();
            let seq = self.next_message_seq(to_session).await?;
            let new_id = MessageId::new();

            sqlx::query(
                r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, ?, ?, ?)"#,
            )
            .bind(new_id.to_string())
            .bind(to_session.to_string())
            .bind(format!("{:?}", msg.role).to_lowercase())
            .bind(seq)
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
                        .execute(&self.pool)
                        .await
                        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn delete_messages_after_seq(
        &self,
        session_id: SessionId,
        cutoff_seq: i64,
    ) -> Result<()> {
        let ids: Vec<String> =
            sqlx::query_scalar(r#"SELECT id FROM messages WHERE session_id = ? AND seq > ?"#)
                .bind(session_id.to_string())
                .bind(cutoff_seq)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        for id in ids {
            sqlx::query(r#"DELETE FROM message_parts WHERE message_id = ?"#)
                .bind(&id)
                .execute(&self.pool)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
            sqlx::query(r#"DELETE FROM messages WHERE id = ?"#)
                .bind(&id)
                .execute(&self.pool)
                .await
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn insert_compaction(
        &self,
        session_id: SessionId,
        message_id: MessageId,
        summary: &str,
        recent: &str,
    ) -> Result<i64> {
        let now = Utc::now();
        let seq = self.next_message_seq(session_id).await?;
        let payload = serde_json::json!({ "summary": summary, "recent": recent });
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at) VALUES (?, ?, 'compaction', ?, ?)"#,
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        sqlx::query(
            r#"INSERT INTO message_parts (message_id, part_index, kind, text) VALUES (?, 0, 'text', ?)"#,
        )
        .bind(message_id.to_string())
        .bind(payload.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
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
            id: SessionId::from_str(&self.id)
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
            location_id: LocationId::from_str(&self.location_id)
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
            title: self.title,
            agent: parse_agent(&self.agent),
            model_id: self.model_id,
            provider_id: self.provider_id,
            parent_session_id: self
                .parent_session_id
                .as_deref()
                .map(SessionId::from_str)
                .transpose()
                .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?,
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
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
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
