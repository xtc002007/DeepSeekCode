//! Session execution with child-session lifecycle management.

use async_trait::async_trait;
use code_plus_plus_config::CodePlusPlusConfig;
use code_plus_plus_schema::event::EventKind;
use code_plus_plus_schema::session::PromptDelivery;
use code_plus_plus_types::{MessageId, Result, SessionId};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use super::runner::SessionRunner;
use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::tools::services::TaskSpawner;
use code_plus_plus_plugin::HookRegistry;
use code_plus_plus_storage::{admit_prompt, Database, EventStore, SessionStore};

pub struct SessionExecution {
    db: Arc<Database>,
    project_dir: String,
    config: CodePlusPlusConfig,
    permissions: Arc<PermissionService>,
    providers: Arc<ProviderCatalog>,
    events: Arc<EventStore>,
    mcp: Arc<McpConnectionPool>,
    hooks: Arc<HookRegistry>,
    sessions: SessionStore,
    runners: Mutex<HashMap<SessionId, Arc<SessionRunner>>>,
    question_channels: Mutex<
        HashMap<
            SessionId,
            Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>>,
        >,
    >,
    child_sessions: Mutex<HashMap<SessionId, Vec<SessionId>>>,
    drain_handles: Mutex<HashMap<SessionId, (tokio::task::Id, tokio::task::JoinHandle<()>)>>,
    /// Sessions whose current run cycle was cancelled by the user. Unlike the
    /// per-runner flag (reset on every `drain()` entry), this survives runner
    /// recreation between drain-loop iterations, so an interrupt can never be
    /// swallowed by a fresh runner starting mid-cycle. Cleared on `wake`.
    interrupt_requested: tokio::sync::Mutex<HashSet<SessionId>>,
    active_session_ids: Option<Arc<tokio::sync::RwLock<HashSet<SessionId>>>>,
    pub(crate) drain_active: Arc<RwLock<HashSet<SessionId>>>,
    pub(crate) drain_needs_rerun: Arc<RwLock<HashSet<SessionId>>>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Database>,
        config: CodePlusPlusConfig,
        project_dir: String,
        permissions: Arc<PermissionService>,
        providers: Arc<ProviderCatalog>,
        events: Arc<EventStore>,
        mcp: Arc<McpConnectionPool>,
        hooks: Arc<HookRegistry>,
        active_session_ids: Option<Arc<tokio::sync::RwLock<HashSet<SessionId>>>>,
        sessions: SessionStore,
        drain_active: Arc<RwLock<HashSet<SessionId>>>,
        drain_needs_rerun: Arc<RwLock<HashSet<SessionId>>>,
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
            sessions,
            runners: Mutex::new(HashMap::new()),
            question_channels: Mutex::new(HashMap::new()),
            child_sessions: Mutex::new(HashMap::new()),
            drain_handles: Mutex::new(HashMap::new()),
            interrupt_requested: tokio::sync::Mutex::new(HashSet::new()),
            active_session_ids,
            drain_active,
            drain_needs_rerun,
        }
    }

    async fn request_interrupt(&self, session_id: SessionId) {
        self.interrupt_requested.lock().await.insert(session_id);
    }

    pub(crate) async fn is_interrupt_requested(&self, session_id: SessionId) -> bool {
        self.interrupt_requested.lock().await.contains(&session_id)
    }

    async fn clear_interrupt_request(&self, session_id: SessionId) {
        self.interrupt_requested.lock().await.remove(&session_id);
    }

    /// Snapshot of location-tracked active ids for SessionService::active()
    /// union. Never blocks on a poisoned std lock (uses tokio mutex).
    pub(crate) async fn snapshot_active_ids(&self) -> Vec<SessionId> {
        if let Some(active) = &self.active_session_ids {
            return active.read().await.iter().copied().collect();
        }
        Vec::new()
    }

    /// Get-or-create the live runner for a session. Shared by `run` and
    /// manual compaction so both build provider-visible context (system,
    /// messages, tools) from the same live registries — never a hollow
    /// context (P0-2).
    async fn runner_for(self: &Arc<Self>, session_id: SessionId) -> Arc<SessionRunner> {
        let spawner: Arc<dyn TaskSpawner> = Arc::new(ExecutionTaskSpawner {
            execution: Arc::clone(self),
        });
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
                    spawner.clone(),
                ))
            })
            .clone()
    }

    /// Manual `/compact` through the live runner: one forced checkpoint with
    /// real baseline/messages/tools, so the stored lineage matches subsequent
    /// turns and validates on read (P0-2). Returns whether a checkpoint was
    /// installed. History is appended, never rewritten.
    pub async fn compact_manual(self: &Arc<Self>, session_id: SessionId) -> Result<bool> {
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "execution compact_manual started"
        );
        let runner = self.runner_for(session_id).await;
        let out = runner.compact_manual_turn().await;
        match &out {
            Ok(installed) => tracing::info!(
                target: code_plus_plus_telemetry::SESSION,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %session_id,
                installed = *installed,
                "execution compact_manual completed"
            ),
            Err(e) => tracing::error!(
                target: code_plus_plus_telemetry::SESSION,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %session_id,
                error = %e,
                "execution compact_manual failed"
            ),
        }
        out
    }

    pub async fn wake(self: &Arc<Self>, session_id: SessionId) -> Result<()> {
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "execution wake started"
        );
        // A fresh user prompt starts a new run cycle: drop any leftover
        // interrupt request from a previous cancelled cycle.
        self.clear_interrupt_request(session_id).await;
        {
            let mut active = self.drain_active.write().await;
            if !active.insert(session_id) {
                self.drain_needs_rerun.write().await.insert(session_id);
                tracing::debug!(
                    target: code_plus_plus_telemetry::SESSION,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    session_id = %session_id,
                    "session already draining; marked for rerun"
                );
                return Ok(());
            }
        }
        let handle = self.spawn_drain(session_id);
        // Abort any stale finished handle before overwriting so detached
        // tasks do not leak (P0-4). Store task Id for still-ours cleanup.
        let id = handle.id();
        if let Some((_, old)) = self
            .drain_handles
            .lock()
            .await
            .insert(session_id, (id, handle))
        {
            old.abort();
        }
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "execution wake completed"
        );
        Ok(())
    }

    fn spawn_drain(self: &Arc<Self>, session_id: SessionId) -> tokio::task::JoinHandle<()> {
        let execution = Arc::clone(self);
        let active = self.drain_active.clone();
        let needs_rerun = self.drain_needs_rerun.clone();
        let events = self.events.clone();
        // P3-2: task-locals do not cross `spawn`; capture the originating
        // HTTP `request_id` so drain logs join 1:1 (session_id remains the
        // primary key for multi-prompt drains).
        let request_id = code_plus_plus_telemetry::current_request_id();
        tokio::spawn(code_plus_plus_telemetry::scope_request_id(
            request_id,
            async move {
                loop {
                    // An interrupt may land between drain iterations; honor it
                    // instead of starting another run with a recreated runner.
                    if execution.is_interrupt_requested(session_id).await {
                        break;
                    }
                    let result = execution.run(session_id).await;
                    let interrupted = matches!(
                        result,
                        Err(code_plus_plus_types::CodePlusPlusError::SessionInterrupted)
                    );
                    // ContextUnavailable (mandatory context missing) blocks the turn
                    // but is NOT an interrupt: keep prompt pending/retryable, no event.
                    let blocked = matches!(
                        result,
                        Err(code_plus_plus_types::CodePlusPlusError::ContextUnavailable(_))
                    );
                    if let Err(e) = result {
                        if interrupted {
                            tracing::debug!(
                                target: code_plus_plus_telemetry::SESSION,
                                run_id = %code_plus_plus_telemetry::current_run_id(),
                                session_id = %session_id,
                                "session drain interrupted"
                            );
                        } else if blocked {
                            tracing::info!(
                                target: code_plus_plus_telemetry::SESSION,
                                run_id = %code_plus_plus_telemetry::current_run_id(),
                                session_id = %session_id,
                                error = %e,
                                "session drain blocked on mandatory context; prompt stays pending"
                            );
                        } else {
                            tracing::error!(
                                target: code_plus_plus_telemetry::SESSION,
                                run_id = %code_plus_plus_telemetry::current_run_id(),
                                session_id = %session_id,
                                error = %e,
                                "session drain error"
                            );
                            // Non-interrupt errors must not masquerade as
                            // SessionInterrupted; surface as a plain error event
                            // payload on SessionIdle path or just break.
                            //
                            // The run IS over, so announce idle exactly like the
                            // success path does. Breaking without it left the UI
                            // pinned on "Running tool: …" with no way to recover.
                            let _ = events
                                .append(EventKind::SessionIdle, Some(session_id), json!({}))
                                .await;
                        }
                        break;
                    }
                    let should_rerun = needs_rerun.write().await.remove(&session_id)
                        || execution
                            .sessions
                            .has_unpromoted_pending(session_id)
                            .await
                            .unwrap_or(false);
                    if !should_rerun {
                        // Don't announce idle for a cycle the user just cancelled.
                        if execution.is_interrupt_requested(session_id).await {
                            break;
                        }
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
                        break;
                    }
                }
                active.write().await.remove(&session_id);
                // Drain finished: remove our own handle entry if it is still ours
                // so `drain_handles` does not grow unbounded (P0-4). `drop_runner`
                // / `wait_children` remove+await the live entry when present.
                // Identity via task Id: never remove a newer drain started after us.
                {
                    let my_id = tokio::task::id();
                    let mut handles = execution.drain_handles.lock().await;
                    if let Some((stored_id, _)) = handles.get(&session_id) {
                        if *stored_id == my_id {
                            handles.remove(&session_id);
                        }
                    }
                }
            },
        ))
    }

    pub async fn run(self: &Arc<Self>, session_id: SessionId) -> Result<()> {
        tracing::debug!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "execution run started"
        );
        // A cancelled cycle must not (re)enter a provider turn. Both checks
        // close the race where `interrupt()` lands while this function is
        // between its entry check and runner creation.
        if self.is_interrupt_requested(session_id).await {
            return Err(code_plus_plus_types::CodePlusPlusError::SessionInterrupted);
        }
        // RAII guard: `runner_for` or a panic between insert/remove must not
        // leave a stale id in `active_session_ids` (previously `active()`
        // reported phantom sessions after abnormal exits).
        struct ActiveGuard<'a> {
            set: Option<&'a tokio::sync::RwLock<std::collections::HashSet<SessionId>>>,
            id: SessionId,
        }
        impl Drop for ActiveGuard<'_> {
            fn drop(&mut self) {
                if let Some(set) = self.set {
                    // Best-effort synchronous cleanup; the async path below
                    // removes it first in the normal case.
                    if let Ok(mut guard) = set.try_write() {
                        guard.remove(&self.id);
                    }
                }
            }
        }
        let _active_guard = ActiveGuard {
            set: self.active_session_ids.as_deref(),
            id: session_id,
        };
        if let Some(active) = &self.active_session_ids {
            active.write().await.insert(session_id);
        }
        let runner = self.runner_for(session_id).await;
        if self.is_interrupt_requested(session_id).await {
            // Drop the freshly (re)created runner so the next cycle starts with
            // a clean interrupted flag instead of draining once more.
            self.runners.lock().await.remove(&session_id);
            return Err(code_plus_plus_types::CodePlusPlusError::SessionInterrupted);
        }
        self.question_channels
            .lock()
            .await
            .insert(session_id, runner.question_channel());
        let result = runner.drain().await;
        self.question_channels.lock().await.remove(&session_id);
        self.wait_children(session_id).await;
        if let Some(active) = &self.active_session_ids {
            active.write().await.remove(&session_id);
        }
        result
    }

    pub async fn spawn_child_task(
        self: &Arc<Self>,
        parent_id: SessionId,
        agent: &str,
        prompt: &str,
    ) -> Result<SessionId> {
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            parent_id = %parent_id,
            agent = %agent,
            prompt = %code_plus_plus_telemetry::summarize_prompt(prompt, false),
            "execution spawn_child_task started"
        );
        let sessions = SessionStore::new(self.db.pool().clone());
        let parent = sessions.get(parent_id).await?;
        let child_id = SessionId::new();
        // Unknown agents are rejected instead of silently becoming Build
        // (consistent with `POST /api/v1/session/{id}/model`).
        let agent_kind = match agent.to_lowercase().as_str() {
            "build" => code_plus_plus_schema::agent::AgentKind::Build,
            "plan" => code_plus_plus_schema::agent::AgentKind::Plan,
            "general" => code_plus_plus_schema::agent::AgentKind::General,
            other => {
                return Err(code_plus_plus_types::CodePlusPlusError::InvalidInput(
                    format!("unknown agent: {other}"),
                ))
            }
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
            .map_err(|e| code_plus_plus_types::CodePlusPlusError::Storage(e.to_string()))?;

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

        Arc::clone(self).wake(child_id).await?;

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
            // Child tasks are spawned via `wake()` and tracked in
            // `drain_handles` like any other drain; no second map exists.
            if let Some((_, handle)) = self.drain_handles.lock().await.remove(&child_id) {
                let _ = handle.await;
            }
        }
    }

    pub async fn submit_question_answer(
        &self,
        session_id: SessionId,
        answer: String,
    ) -> Result<()> {
        // Clone the Arcs out of the maps first: holding `runners` across
        // `submit_question_answer` (which locks the question channel) while
        // `interrupt` locks in the opposite order would deadlock.
        let runner = self.runners.lock().await.get(&session_id).cloned();
        if let Some(runner) = runner {
            return runner.submit_question_answer(answer).await;
        }
        let channel = self
            .question_channels
            .lock()
            .await
            .get(&session_id)
            .cloned();
        if let Some(channel) = channel {
            return crate::tools::services::submit_question_answer_channel(&channel, answer).await;
        }
        Err(code_plus_plus_types::CodePlusPlusError::NotFound(
            "session runner not active".into(),
        ))
    }

    pub async fn interrupt(&self, session_id: SessionId) -> Result<()> {
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "execution interrupt started"
        );
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
            // Durable per-cycle marker: survives runner recreation between
            // drain iterations, so a later drain pass cannot resurrect the run.
            self.request_interrupt(sid).await;
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
        // Awaiting (not aborting) is intentional: callers (move/delete) need
        // the drain to actually stop before mutating session state, and the
        // interrupt above guarantees the drain exits at its next boundary
        // instead of hanging. Aborting mid-DB-write could leave partial rows.
        if let Some((_, handle)) = self.drain_handles.lock().await.remove(&session_id) {
            // Bound the wait: interrupt guarantees prompt exit, but a stuck
            // provider stream (90s idle timeout) must not block move/delete
            // for the full timeout. Abort after 5s grace.
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        target: code_plus_plus_telemetry::SESSION,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        session_id = %session_id,
                        "drop_runner drain did not exit in 5s; aborting"
                    );
                }
            }
        }
    }

    pub async fn refresh_mcp_tools_for_all_runners(&self, config: &CodePlusPlusConfig) {
        let runners = self.runners.lock().await;
        for runner in runners.values() {
            runner.refresh_mcp_tools(config).await;
        }
    }

    pub async fn reload_materialization_for_all_runners(&self, config: &CodePlusPlusConfig) {
        let runners = self.runners.lock().await;
        for runner in runners.values() {
            runner.reload_materialization(config).await;
        }
    }
}
