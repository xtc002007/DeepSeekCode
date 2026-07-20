use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest, Session};
use deepseekcode_schema::plan::SessionPlanSummary;
use deepseekcode_storage::{admit_prompt, Database, EventStore, PlanStore, SessionStore, TodoStore};
use deepseekcode_types::{LocationId, MessageId, Result, SessionId};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::execution::SessionExecution;
use super::projector::EventProjector;
use super::revert::RevertManager;
use crate::activity::{emit_activity_updated, ActivityProjector, ChildSessionProjection};
use crate::mcp::McpConnectionPool;
use crate::provider::ProviderCatalog;
use crate::system_context::SystemContextRegistry;
use deepseekcode_plugin::HookRegistry;

pub struct SessionService {
    db: Arc<Database>,
    pub(crate) sessions: SessionStore,
    pub(crate) events: Arc<EventStore>,
    execution: Arc<SessionExecution>,
    active: Arc<RwLock<HashSet<SessionId>>>,
    config: DeepSeekCodeConfig,
    project_dir: String,
    providers: Arc<ProviderCatalog>,
}

impl SessionService {
    pub fn new(
        db: Arc<Database>,
        config: DeepSeekCodeConfig,
        project_dir: String,
        permissions: Arc<crate::permission::PermissionService>,
        providers: Arc<ProviderCatalog>,
        events: Arc<EventStore>,
        mcp: Arc<McpConnectionPool>,
        hooks: Arc<HookRegistry>,
        active_session_id: Option<Arc<std::sync::RwLock<Option<deepseekcode_types::SessionId>>>>,
    ) -> Self {
        let pool = db.pool().clone();
        Self {
            sessions: SessionStore::new(pool.clone()),
            events: events.clone(),
            execution: Arc::new(SessionExecution::new(
                db.clone(),
                config.clone(),
                project_dir.clone(),
                permissions,
                providers.clone(),
                events,
                mcp,
                hooks,
                active_session_id,
            )),
            active: Arc::new(RwLock::new(HashSet::new())),
            db,
            config,
            project_dir,
            providers: providers.clone(),
        }
    }

    pub fn providers(&self) -> Arc<ProviderCatalog> {
        self.providers.clone()
    }

    pub fn event_store(&self) -> Arc<EventStore> {
        self.events.clone()
    }

    pub async fn refresh_mcp_tools(&self) {
        self.execution.refresh_mcp_tools_for_all_runners().await;
    }

    pub async fn reload_materialization(&self) {
        self.execution
            .reload_materialization_for_all_runners(&self.config)
            .await;
    }

    pub async fn create(&self, req: CreateSessionRequest) -> Result<Session> {
        let id = req.id.unwrap_or_default();
        if let Some(existing) = self.sessions.find(id).await? {
            return Ok(existing);
        }

        let location_id = LocationId::new();
        let agent = req.agent.unwrap_or_default();
        let model_id = req
            .model_id
            .or_else(|| self.config.model.as_ref().and_then(|m| m.id.clone()));
        let provider_id = req
            .provider_id
            .or_else(|| self.config.model.as_ref().and_then(|m| m.provider.clone()));

        let session = self
            .sessions
            .create(
                id,
                location_id,
                &req.directory,
                agent,
                model_id.as_deref(),
                provider_id.as_deref(),
            )
            .await?;

        self.events
            .append(
                EventKind::SessionCreated,
                Some(session.id),
                json!({ "directory": req.directory }),
            )
            .await?;

        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %session.id,
            directory = %req.directory,
            "session created"
        );

        Ok(session)
    }

    pub async fn prompt(&self, req: PromptRequest) -> Result<MessageId> {
        self.ensure_session_in_location(req.session_id).await?;
        let message_id = req.id.unwrap_or_default();

        let mut tx = self.db.pool().begin().await.map_err(|e| {
            deepseekcode_types::DeepSeekCodeError::Storage(e.to_string())
        })?;
        let inserted = sqlx::query(
            r#"INSERT OR IGNORE INTO session_input (id, session_id, prompt, delivery, admitted_at, promoted)
               VALUES (?, ?, ?, ?, ?, 0)"#,
        )
        .bind(message_id.to_string())
        .bind(req.session_id.to_string())
        .bind(&req.prompt)
        .bind(match req.delivery {
            PromptDelivery::Steer => "steer",
            PromptDelivery::Queue => "queue",
        })
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;

        if inserted.rows_affected() == 0 {
            tx.commit()
                .await
                .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;
            return Ok(message_id);
        }
        tx.commit()
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;

        self.events
            .append(
                EventKind::PromptAdmitted,
                Some(req.session_id),
                json!({
                    "message_id": message_id.to_string(),
                    "prompt": req.prompt,
                    "delivery": format!("{:?}", req.delivery).to_lowercase(),
                }),
            )
            .await?;

        if req.resume {
            self.wake(req.session_id).await?;
        }

        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %req.session_id,
            message_id = %message_id,
            prompt = %deepseekcode_telemetry::summarize_prompt(
                &req.prompt,
                self.config.logging.audit.include_prompts
            ),
            "prompt admitted"
        );

        Ok(message_id)
    }

    pub async fn interrupt(&self, session_id: SessionId) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.execution.drop_runner(session_id).await;
        self.active.write().await.remove(&session_id);
        self.events
            .append(EventKind::SessionInterrupted, Some(session_id), json!({}))
            .await?;
        Ok(())
    }

    pub async fn latest_event_seq(&self, session_id: SessionId) -> Result<i64> {
        self.ensure_session_in_location(session_id).await?;
        self.events.latest_seq(session_id).await
    }

    pub async fn delete(&self, session_id: SessionId) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.execution.drop_runner(session_id).await;
        self.active.write().await.remove(&session_id);
        self.sessions.delete(session_id).await
    }

    pub async fn active(&self) -> Result<Vec<SessionId>> {
        Ok(self.active.read().await.iter().copied().collect())
    }

    pub async fn list(&self) -> Result<Vec<Session>> {
        self.sessions
            .list_for_working_directory(&self.project_dir)
            .await
    }

    pub async fn get(&self, session_id: SessionId) -> Result<Session> {
        self.ensure_session_in_location(session_id).await
    }

    /// Returns the session when it belongs to this location's working directory.
    pub async fn ensure_session_in_location(&self, session_id: SessionId) -> Result<Session> {
        let session = self.sessions.get(session_id).await?;
        let working_directory: Option<String> = sqlx::query_scalar(
            r#"SELECT working_directory FROM sessions WHERE id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;
        let working_directory = working_directory.ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::NotFound(format!("session {session_id} not found"))
        })?;
        if !session_belongs_to_location(&working_directory, &self.project_dir) {
            return Err(deepseekcode_types::DeepSeekCodeError::NotFound(format!(
                "session {session_id} not found in this project"
            )));
        }
        Ok(session)
    }

    pub async fn list_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::message::Message>> {
        self.ensure_session_in_location(session_id).await?;
        EventProjector::project_messages(&self.sessions, self.events.as_ref(), session_id).await
    }

    pub fn db_pool(&self) -> &sqlx::SqlitePool {
        self.db.pool()
    }

    pub async fn fork(
        &self,
        session_id: SessionId,
        until_message_id: Option<MessageId>,
    ) -> Result<Session> {
        let original = self.ensure_session_in_location(session_id).await?;
        let new_id = SessionId::new();
        let session = self
            .sessions
            .create(
                new_id,
                original.location_id,
                &self.project_dir,
                original.agent,
                original.model_id.as_deref(),
                original.provider_id.as_deref(),
            )
            .await?;

        match until_message_id {
            Some(until) => {
                self.sessions
                    .copy_messages_until(session_id, new_id, until)
                    .await?;
            }
            None => {
                self.sessions
                    .copy_messages_with_parts(session_id, new_id)
                    .await?;
            }
        }

        self.events
            .append(
                EventKind::SessionCreated,
                Some(new_id),
                json!({
                    "forked_from": session_id.to_string(),
                    "until_message_id": until_message_id.map(|m| m.to_string()),
                }),
            )
            .await?;

        Ok(session)
    }

    pub async fn export_session(
        &self,
        session_id: SessionId,
        sanitize: bool,
    ) -> Result<serde_json::Value> {
        let session = self.ensure_session_in_location(session_id).await?;
        let mut messages =
            EventProjector::project_messages(&self.sessions, self.events.as_ref(), session_id)
                .await?;
        if sanitize {
            messages = sanitize_messages(messages);
        }
        let timeline = EventProjector::project_events(self.events.as_ref(), session_id, 0).await?;
        Ok(json!({
            "version": 2,
            "session": session,
            "messages": messages,
            "timeline": timeline,
        }))
    }

    pub async fn list_timeline(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<Vec<super::projector::TimelineEntry>> {
        self.ensure_session_in_location(session_id).await?;
        EventProjector::project_events(self.events.as_ref(), session_id, since_seq).await
    }

    pub async fn list_events(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<Vec<deepseekcode_schema::event::EventEnvelope>> {
        self.ensure_session_in_location(session_id).await?;
        self.events
            .tail_filtered(since_seq, 10_000, Some(session_id))
            .await
    }

    pub async fn list_activity(
        &self,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<deepseekcode_schema::activity::ActivityTree> {
        self.ensure_session_in_location(session_id).await?;
        let events = self
            .events
            .tail_filtered(since_seq, 10_000, Some(session_id))
            .await?;
        let messages = self.list_messages(session_id).await?;
        let sessions = SessionStore::new(self.db.pool().clone());
        let child_ids = sessions.list_child_sessions(session_id).await?;
        let mut children = std::collections::HashMap::new();
        for child_id in child_ids {
            let child_events = self
                .events
                .tail_filtered(0, 10_000, Some(child_id))
                .await?;
            let child_messages = self.list_messages(child_id).await?;
            children.insert(
                child_id.to_string(),
                ChildSessionProjection {
                    events: child_events,
                    messages: child_messages,
                },
            );
        }
        Ok(ActivityProjector::project_with_children(
            &events,
            &messages,
            &children,
        ))
    }

    pub async fn get_todos(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<deepseekcode_schema::todo::TodoItem>> {
        self.ensure_session_in_location(session_id).await?;
        TodoStore::new(self.db.pool().clone())
            .get(session_id)
            .await
    }

    pub async fn update_todos(
        &self,
        session_id: SessionId,
        todos: &[deepseekcode_schema::todo::TodoItem],
    ) -> Result<Vec<deepseekcode_schema::todo::TodoItem>> {
        self.ensure_session_in_location(session_id).await?;
        let store = TodoStore::new(self.db.pool().clone());
        let saved = store.update(session_id, todos).await?;
        self.events
            .append(
                EventKind::TodoUpdated,
                Some(session_id),
                json!({ "todos": saved }),
            )
            .await?;
        let _ = emit_activity_updated(self.events.as_ref(), session_id, 0).await;
        Ok(saved)
    }

    pub async fn submit_question_answer(
        &self,
        session_id: SessionId,
        answer: String,
    ) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.execution.submit_question_answer(session_id, answer).await
    }

    pub async fn create_child_session(
        &self,
        parent_id: SessionId,
        agent: deepseekcode_schema::agent::AgentKind,
        title: Option<&str>,
    ) -> Result<Session> {
        let parent = self.ensure_session_in_location(parent_id).await?;
        let new_id = SessionId::new();
        let session = self
            .sessions
            .create(
                new_id,
                parent.location_id,
                &self.project_dir,
                agent,
                parent.model_id.as_deref(),
                parent.provider_id.as_deref(),
            )
            .await?;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE sessions SET parent_session_id = ?, title = COALESCE(?, title) WHERE id = ?")
            .bind(parent_id.to_string())
            .bind(title)
            .bind(new_id.to_string())
            .execute(self.db.pool())
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;
        self.events
            .append(
                EventKind::SessionCreated,
                Some(new_id),
                json!({
                    "directory": self.project_dir,
                    "parent_session_id": parent_id.to_string(),
                }),
            )
            .await?;
        Ok(session)
    }

    fn revert_manager(&self) -> RevertManager {
        RevertManager::new(
            self.project_dir.clone(),
            self.db.pool().clone(),
            SessionStore::new(self.db.pool().clone()),
            self.events.clone(),
        )
    }

    pub async fn revert_stage(
        &self,
        session_id: SessionId,
        message_id: MessageId,
    ) -> Result<super::revert::StagedRevert> {
        self.ensure_session_in_location(session_id).await?;
        if self.active.read().await.contains(&session_id) {
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                "session is busy".into(),
            ));
        }
        self.revert_manager().stage(session_id, message_id).await
    }

    pub async fn revert_clear(&self, session_id: SessionId) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.revert_manager().clear(session_id).await
    }

    pub async fn revert_commit(
        &self,
        session_id: SessionId,
    ) -> Result<super::revert::StagedRevert> {
        self.ensure_session_in_location(session_id).await?;
        if self.active.read().await.contains(&session_id) {
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                "session is busy".into(),
            ));
        }
        self.revert_manager().commit(session_id).await
    }

    pub async fn revert_get_staged(
        &self,
        session_id: SessionId,
    ) -> Result<Option<super::revert::StagedRevert>> {
        self.ensure_session_in_location(session_id).await?;
        self.revert_manager().get_staged(session_id).await
    }

    pub async fn move_session(&self, session_id: SessionId, new_directory: String) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.execution.drop_runner(session_id).await;
        use crate::system_context::ContextEpochStore;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE sessions SET working_directory = ?, updated_at = ? WHERE id = ?"#,
        )
        .bind(&new_directory)
        .bind(&now)
        .bind(session_id.to_string())
        .execute(self.db.pool())
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;

        let epochs = ContextEpochStore::new(self.db.pool().clone());
        epochs.clear(session_id).await?;

        let registry = SystemContextRegistry::with_config(&new_directory, &self.config);
        registry
            .ensure_epoch(&epochs, self.events.as_ref(), session_id)
            .await?;

        self.events
            .append(
                EventKind::ContextUpdated,
                Some(session_id),
                json!({ "moved_to": new_directory, "epoch_cleared": true }),
            )
            .await?;
        Ok(())
    }

    pub async fn update_model(
        &self,
        session_id: SessionId,
        agent: deepseekcode_schema::agent::AgentKind,
        model_id: String,
        provider_id: String,
    ) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        self.execution.drop_runner(session_id).await;
        self.sessions
            .update_session_model(session_id, agent, &model_id, &provider_id)
            .await
    }

    pub async fn update_session(
        &self,
        session_id: SessionId,
        title: Option<String>,
    ) -> Result<()> {
        self.ensure_session_in_location(session_id).await?;
        if let Some(title) = title {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                self.sessions.update_title(session_id, trimmed).await?;
            }
        }
        Ok(())
    }

    pub async fn list_session_plans(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<SessionPlanSummary>> {
        self.ensure_session_in_location(session_id).await?;
        let events = self
            .events
            .tail_filtered(0, 10_000, Some(session_id))
            .await?;
        let dismissed = PlanStore::new(self.db.pool().clone())
            .list_dismissed(session_id)
            .await?;
        Ok(crate::plan::session_plans_from_events(&events, &dismissed))
    }

    pub async fn dismiss_plan_implement_prompt(
        &self,
        session_id: SessionId,
        plan_id: &str,
    ) -> Result<()> {
        PlanStore::new(self.db.pool().clone())
            .upsert_dismissed(session_id, plan_id, true)
            .await
    }

    async fn wake(&self, session_id: SessionId) -> Result<()> {
        {
            let mut active = self.active.write().await;
            if !active.insert(session_id) {
                tracing::debug!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %session_id,
                    "session already draining"
                );
                return Ok(());
            }
        }
        let execution = self.execution.clone();
        let active = self.active.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let result = execution.run(session_id).await;
            if let Err(e) = result {
                // User interrupt already emits SessionInterrupted in `interrupt()`.
                if matches!(e, deepseekcode_types::DeepSeekCodeError::SessionInterrupted) {
                    tracing::debug!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %session_id,
                        "session drain interrupted"
                    );
                } else {
                    tracing::error!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %session_id,
                        error = %e,
                        "session drain error"
                    );
                    let _ = events
                        .append(
                            EventKind::SessionInterrupted,
                            Some(session_id),
                            json!({ "error": e.to_string() }),
                        )
                        .await;
                }
            } else {
                let _ = events
                    .append(EventKind::SessionIdle, Some(session_id), json!({}))
                    .await;
                if let Ok(seq) = events.latest_seq(session_id).await {
                    let _ = events
                        .append(
                            EventKind::ActivityUpdated,
                            Some(session_id),
                            json!({ "since_seq": seq }),
                        )
                        .await;
                }
            }
            active.write().await.remove(&session_id);
        });
        Ok(())
    }
}

fn sanitize_messages(
    messages: Vec<deepseekcode_schema::message::Message>,
) -> Vec<deepseekcode_schema::message::Message> {
    use deepseekcode_schema::message::PartKind;
    messages
        .into_iter()
        .map(|mut message| {
            for part in &mut message.parts {
                match part.kind {
                    PartKind::ToolCall => {
                        part.tool_input = Some(json!("[redacted]"));
                    }
                    PartKind::ToolResult => {
                        part.tool_output = Some(json!("[redacted]"));
                    }
                    PartKind::Text | PartKind::Reasoning => {
                        if part.text.as_ref().map(|t| t.len()).unwrap_or(0) > 200 {
                            part.text = Some("[redacted]".into());
                        }
                    }
                }
            }
            message
        })
        .collect()
}

fn session_belongs_to_location(session_dir: &str, project_dir: &str) -> bool {
    fn normalize(path: &str) -> String {
        path.trim_end_matches(['/', '\\'])
            .replace('\\', "/")
            .to_lowercase()
    }
    normalize(session_dir) == normalize(project_dir)
}
