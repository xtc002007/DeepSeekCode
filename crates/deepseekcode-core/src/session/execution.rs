//! Session execution with child-session lifecycle management.

use async_trait::async_trait;
use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::session::PromptDelivery;
use deepseekcode_types::{MessageId, Result, SessionId};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::runner::SessionRunner;
use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::tools::services::TaskSpawner;
use deepseekcode_plugin::HookRegistry;
use deepseekcode_storage::{admit_prompt, Database, EventStore, SessionStore};

pub struct SessionExecution {
    db: Arc<Database>,
    project_dir: String,
    config: DeepSeekCodeConfig,
    permissions: Arc<PermissionService>,
    providers: Arc<ProviderCatalog>,
    events: Arc<EventStore>,
    mcp: Arc<McpConnectionPool>,
    hooks: Arc<HookRegistry>,
    runners: Mutex<HashMap<SessionId, Arc<SessionRunner>>>,
    question_channels: Mutex<
        HashMap<
            SessionId,
            Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>>,
        >,
    >,
    child_sessions: Mutex<HashMap<SessionId, Vec<SessionId>>>,
    child_handles: Mutex<HashMap<SessionId, tokio::task::JoinHandle<()>>>,
    active_session_id: Option<Arc<std::sync::RwLock<Option<SessionId>>>>,
}

struct ExecutionTaskSpawner {
    execution: Arc<SessionExecution>,
}

#[async_trait]
impl TaskSpawner for ExecutionTaskSpawner {
    async fn spawn_task(
        &self,
        parent_session_id: SessionId,
        agent: &str,
        prompt: &str,
    ) -> Result<SessionId> {
        SessionExecution::spawn_child_task(&self.execution, parent_session_id, agent, prompt).await
    }
}

impl SessionExecution {
    pub fn new(
        db: Arc<Database>,
        config: DeepSeekCodeConfig,
        project_dir: String,
        permissions: Arc<PermissionService>,
        providers: Arc<ProviderCatalog>,
        events: Arc<EventStore>,
        mcp: Arc<McpConnectionPool>,
        hooks: Arc<HookRegistry>,
        active_session_id: Option<Arc<std::sync::RwLock<Option<SessionId>>>>,
    ) -> Self {
        Self {
            db,
            project_dir,
            config,
            permissions,
            providers,
            events,
            mcp,
            hooks,
            runners: Mutex::new(HashMap::new()),
            question_channels: Mutex::new(HashMap::new()),
            child_sessions: Mutex::new(HashMap::new()),
            child_handles: Mutex::new(HashMap::new()),
            active_session_id,
        }
    }

    pub async fn run(self: &Arc<Self>, session_id: SessionId) -> Result<()> {
        if let Some(active) = &self.active_session_id {
            if let Ok(mut guard) = active.write() {
                *guard = Some(session_id);
            }
        }
        let spawner: Arc<dyn TaskSpawner> = Arc::new(ExecutionTaskSpawner {
            execution: Arc::clone(self),
        });
        let runner = {
            let mut runners = self.runners.lock().await;
            runners
                .entry(session_id)
                .or_insert_with(|| {
                    Arc::new(SessionRunner::new(
                        self.db.clone(),
                        session_id,
                        self.project_dir.clone(),
                        self.config.clone(),
                        self.permissions.clone(),
                        self.providers.clone(),
                        self.events.clone(),
                        self.mcp.clone(),
                        self.hooks.clone(),
                        spawner,
                    ))
                })
                .clone()
        };
        self.question_channels
            .lock()
            .await
            .insert(session_id, runner.question_channel());
        let result = runner.drain().await;
        self.question_channels.lock().await.remove(&session_id);
        self.wait_children(session_id).await;
        if let Some(active) = &self.active_session_id {
            if let Ok(mut guard) = active.write() {
                if guard.as_ref() == Some(&session_id) {
                    *guard = None;
                }
            }
        }
        result
    }

    pub async fn spawn_child_task(
        self: &Arc<Self>,
        parent_id: SessionId,
        agent: &str,
        prompt: &str,
    ) -> Result<SessionId> {
        let sessions = SessionStore::new(self.db.pool().clone());
        let parent = sessions.get(parent_id).await?;
        let child_id = SessionId::new();
        let agent_kind = match agent.to_lowercase().as_str() {
            "plan" => deepseekcode_schema::agent::AgentKind::Plan,
            "general" => deepseekcode_schema::agent::AgentKind::General,
            _ => deepseekcode_schema::agent::AgentKind::Build,
        };

        sessions
            .create(
                child_id,
                parent.location_id,
                &self.project_dir,
                agent_kind,
                parent.model_id.as_deref(),
                parent.provider_id.as_deref(),
            )
            .await?;

        sqlx::query("UPDATE sessions SET parent_session_id = ? WHERE id = ?")
            .bind(parent_id.to_string())
            .bind(child_id.to_string())
            .execute(self.db.pool())
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;

        self.events
            .append(
                EventKind::SessionCreated,
                Some(child_id),
                json!({
                    "directory": self.project_dir,
                    "parent_session_id": parent_id.to_string(),
                    "agent": agent,
                }),
            )
            .await?;

        let message_id = MessageId::new();
        admit_prompt(
            self.db.pool(),
            message_id,
            child_id,
            prompt,
            PromptDelivery::Queue,
        )
        .await?;

        self.events
            .append(
                EventKind::PromptAdmitted,
                Some(child_id),
                json!({
                    "message_id": message_id.to_string(),
                    "delivery": "queue",
                }),
            )
            .await?;

        self.child_sessions
            .lock()
            .await
            .entry(parent_id)
            .or_default()
            .push(child_id);

        let execution = Arc::clone(self);
        let events = self.events.clone();
        let handle = tokio::spawn(async move {
            let result = execution.run(child_id).await;
            if result.is_ok() {
                let _ = events
                    .append(EventKind::SessionIdle, Some(child_id), json!({}))
                    .await;
            } else if let Err(e) = result {
                let _ = events
                    .append(
                        EventKind::SessionInterrupted,
                        Some(child_id),
                        json!({ "error": e.to_string() }),
                    )
                    .await;
            }
            execution.child_handles.lock().await.remove(&child_id);
        });
        self.child_handles.lock().await.insert(child_id, handle);

        Ok(child_id)
    }

    async fn wait_children(&self, parent_id: SessionId) {
        let children: Vec<SessionId> = self
            .child_sessions
            .lock()
            .await
            .remove(&parent_id)
            .unwrap_or_default();
        for child_id in children {
            if let Some(handle) = self.child_handles.lock().await.remove(&child_id) {
                let _ = handle.await;
            }
        }
    }

    pub async fn submit_question_answer(
        &self,
        session_id: SessionId,
        answer: String,
    ) -> Result<()> {
        let runners = self.runners.lock().await;
        if let Some(runner) = runners.get(&session_id) {
            return runner.submit_question_answer(answer);
        }
        drop(runners);
        let channels = self.question_channels.lock().await;
        if let Some(channel) = channels.get(&session_id) {
            return crate::tools::services::submit_question_answer_channel(channel, answer);
        }
        Err(deepseekcode_types::DeepSeekCodeError::NotFound(
            "session runner not active".into(),
        ))
    }

    pub async fn interrupt(&self, session_id: SessionId) -> Result<()> {
        let mut stack = vec![session_id];
        while let Some(sid) = stack.pop() {
            let children: Vec<SessionId> = self
                .child_sessions
                .lock()
                .await
                .get(&sid)
                .cloned()
                .unwrap_or_default();
            stack.extend(children);
            let runners = self.runners.lock().await;
            if let Some(runner) = runners.get(&sid) {
                runner.interrupt().await;
            }
        }
        Ok(())
    }

    pub async fn drop_runner(&self, session_id: SessionId) {
        let _ = self.interrupt(session_id).await;
        self.runners.lock().await.remove(&session_id);
    }

    pub async fn refresh_mcp_tools_for_all_runners(&self) {
        let runners = self.runners.lock().await;
        for runner in runners.values() {
            runner.refresh_mcp_tools().await;
        }
    }

    pub async fn reload_materialization_for_all_runners(&self, config: &DeepSeekCodeConfig) {
        let runners = self.runners.lock().await;
        for runner in runners.values() {
            runner.reload_materialization(config).await;
        }
    }
}
