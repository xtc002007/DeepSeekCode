use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_llm::{
    is_context_overflow, LlmClient, LlmEvent, LlmMessage, LlmRequest, LlmResponse, LlmRole,
    LlmToolCall,
};

mod context_reconciler;
mod prompt_promoter;
mod repair;
mod tool_executor;
use prompt_promoter::PromptPromoter;
use repair::{
    agent_step_limit, estimate_message_tokens, message_reasoning, message_text, message_tool_calls,
    provider_tool_call_id, repair_llm_tool_chain, should_retry_without_reasoning,
    truncate_tool_content_for_prefix,
};

pub(super) struct ProviderTurnOutcome {
    response: LlmResponse,
    assistant_message_id: MessageId,
}
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::message::{MessageRole, PartKind};
use deepseekcode_storage::{
    Database, EventStore, ManagedToolOutput, SessionStore, ToolOutputLimits,
};
use deepseekcode_types::{
    DeepSeekCodeError, MessageId, Result, SessionId, CONFIG_FILE, PRODUCT_NAME,
};
use futures::StreamExt;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::compaction::{
    compaction_checkpoint_content, parse_compaction_message, verify_checkpoint_coverage,
    CompactionContext, CompactionService, FailedCompaction,
};
use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::session::projector::EventProjector;
use crate::snapshot::FileSnapshotStore;
use crate::system_context::{ContextEpochStore, SystemContextRegistry};
use crate::tools::{CancelToken, EventToolProgressSink, ToolRegistry, ToolServices};
use deepseekcode_plugin::HookRegistry;
use deepseekcode_storage::TodoStore;

/// Issue-6 helper: hex sha256 for system-cache keys.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

const MAX_STEPS_PROMPT: &str = "CRITICAL - MAXIMUM STEPS REACHED\n\n\
The maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.\n\n\
STRICT REQUIREMENTS:\n\
1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)\n\
2. MUST provide a text response summarizing work done so far\n\
3. This constraint overrides ALL other instructions, including any user requests for edits or tool use\n\n\
Response must include:\n\
- Statement that maximum steps for this agent have been reached\n\
- Summary of what has been accomplished so far\n\
- List of any remaining tasks that were not completed\n\
- Recommendations for what should be done next\n\n\
Any attempt to use tools is a critical violation. Respond with text ONLY.";

/// Shared merge for both `transform_system` paths (cached fast path and
/// hook path): baseline + built-in plan prompt + config agent prompt, in that
/// order. Previously duplicated inline with drift risk between the branches.
fn merge_plan_prompt(
    baseline: String,
    agent: &deepseekcode_schema::agent::AgentKind,
    cfg_prompt: &str,
) -> String {
    let mut merged = baseline;
    if matches!(agent, deepseekcode_schema::agent::AgentKind::Plan) {
        let builtin = crate::agent::plan_system_prompt();
        if !builtin.is_empty() {
            merged = format!("{merged}\n\n{builtin}");
        }
    }
    if !cfg_prompt.is_empty() {
        merged = format!("{merged}\n\n{cfg_prompt}");
    }
    merged
}

pub struct SessionRunner {
    session_id: SessionId,
    project_dir: String,
    config: DeepSeekCodeConfig,
    sessions: SessionStore,
    events: Arc<EventStore>,
    context_registry: SystemContextRegistry,
    context_epochs: ContextEpochStore,
    tools: Arc<tokio::sync::RwLock<ToolRegistry>>,
    permissions: Arc<PermissionService>,
    providers: Arc<ProviderCatalog>,
    tool_output: ManagedToolOutput,
    interrupted: Arc<CancelToken>,
    in_provider_turn: AtomicBool,
    strip_reasoning_next_turn: AtomicBool,
    llm: Arc<Mutex<LlmClient>>,
    mcp: Arc<McpConnectionPool>,
    hooks: Arc<HookRegistry>,
    question_tx: Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>>,
    // (fingerprint -> (count, last_user_turn)). Reset on new user promotion
    // so `A,user,A,user,A` is not mis-killed; success also resets.
    loop_guard_counts: Arc<Mutex<std::collections::HashMap<String, (u32, u64)>>>,
    loop_guard_turn: Arc<std::sync::atomic::AtomicU64>,
    /// Issue-6: deterministic `transform_system` cache. Key =
    /// `sha256(baseline)|agent|model|cfg_prompt_hash`. Only used when
    /// `hooks.is_empty()` so dynamic hook output is never frozen.
    /// Bounded to 128 entries (LRU-ish eviction) to avoid unbounded growth
    /// in long-lived processes.
    system_cache: Mutex<std::collections::HashMap<String, String>>,
    /// P1-3: last deterministic compaction failure for this session's view.
    /// Same-view repeats skip the summarizer call (`blocked-same-view`);
    /// progress or success clears it. Threaded into every `CompactionContext`
    /// built here so auto/overflow/manual share one record.
    compaction_failure: Mutex<Option<FailedCompaction>>,
}

impl SessionRunner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Database>,
        session_id: SessionId,
        project_dir: String,
        config: DeepSeekCodeConfig,
        permissions: Arc<PermissionService>,
        providers: Arc<ProviderCatalog>,
        events: Arc<EventStore>,
        mcp: Arc<McpConnectionPool>,
        hooks: Arc<HookRegistry>,
        task_spawner: Arc<dyn crate::tools::services::TaskSpawner>,
    ) -> Self {
        let pool = db.pool().clone();
        let snapshots_enabled = config.snapshots.as_ref().map(|s| s.enabled).unwrap_or(true);
        let snapshot_store = Arc::new(FileSnapshotStore::new(
            pool.clone(),
            project_dir.clone(),
            snapshots_enabled,
        ));
        let todo_store = Arc::new(TodoStore::new(pool.clone()));
        let progress = Arc::new(EventToolProgressSink::new(events.clone()));
        let question_tx = Arc::new(tokio::sync::Mutex::new(None));
        let services = ToolServices {
            todo_store: Some(todo_store),
            events: Some(events.clone()),
            progress: Some(progress),
            question_tx: Some(question_tx.clone()),
            task_spawner: Some(task_spawner),
            cancel: None,
        };
        let mut tools = ToolRegistry::with_services(project_dir.clone(), &config, services)
            .with_snapshot_store(snapshot_store);
        tools.register_extensions(&mcp);
        Self {
            sessions: SessionStore::new(pool.clone()),
            context_epochs: ContextEpochStore::new(pool),
            context_registry: SystemContextRegistry::with_config(&project_dir, &config),
            tools: Arc::new(tokio::sync::RwLock::new(tools)),
            tool_output: ManagedToolOutput::project(&project_dir),
            permissions,
            providers,
            events,
            interrupted: Arc::new(CancelToken::new()),
            in_provider_turn: AtomicBool::new(false),
            strip_reasoning_next_turn: AtomicBool::new(false),
            llm: Arc::new(Mutex::new(LlmClient::new())),
            session_id,
            project_dir,
            config,
            mcp,
            hooks,
            question_tx,
            loop_guard_counts: Arc::new(Mutex::new(std::collections::HashMap::new())),
            loop_guard_turn: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            system_cache: Mutex::new(std::collections::HashMap::new()),
            compaction_failure: Mutex::new(None),
        }
    }

    pub async fn submit_question_answer(&self, answer: String) -> Result<()> {
        crate::tools::services::submit_question_answer(
            &ToolServices {
                question_tx: Some(self.question_tx.clone()),
                ..ToolServices::default()
            },
            answer,
        )
        .await
    }

    pub fn question_channel(
        &self,
    ) -> Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>> {
        self.question_tx.clone()
    }

    pub async fn interrupt(&self) {
        self.interrupted.cancel();
        crate::process::kill_session_children(self.session_id).await;
        // Drop any pending question responder so blocked tools exit promptly.
        let mut question = self.question_tx.lock().await;
        question.take();
        // Abort pending policy prompts for this session (P0-4/P0-8).
        self.permissions.cancel_pending(self.session_id).await;
    }

    /// Test helper: simulate an in-flight provider turn.
    pub fn test_set_in_provider_turn(&self, active: bool) {
        self.in_provider_turn.store(active, Ordering::SeqCst);
    }

    /// Test helper: promote queue/steer prompts at the current safe boundary.
    pub async fn test_promote_at_safe_boundary(&self) -> Result<bool> {
        self.promote_at_safe_boundary().await
    }

    /// Refresh MCP-bridged tools after connect/disconnect.
    pub async fn refresh_mcp_tools(&self) {
        let mut tools = self.tools.write().await;
        tools.refresh_mcp_tools(&self.mcp);
    }

    /// Re-materialize skill tool and system context after catalog changes.
    pub async fn reload_materialization(&self, config: &DeepSeekCodeConfig) {
        let mut tools = self.tools.write().await;
        tools.refresh_skill_tool(config);
    }

    /// Issue-11: provider-visible MCP names. When `direct_tools=false`
    /// (default) MCP lives behind the frozen `use_capability` proxy, so the
    /// live catalog must NOT enter the allowed-filter or lineage hash —
    /// otherwise every MCP connect/disconnect false-positives
    /// `prefix_changed` while the actual `tools` array never moved.
    async fn provider_visible_mcp_tools(&self) -> Vec<String> {
        let direct = self.tools.read().await.mcp_direct();
        if direct {
            self.mcp.list_registered_tool_names()
        } else {
            Vec::new()
        }
    }

    async fn tool_definitions(
        &self,
        agent: &deepseekcode_schema::agent::AgentKind,
    ) -> Vec<deepseekcode_schema::tool::ToolDefinition> {
        let agent_str = format!("{agent:?}").to_lowercase();
        let mcp_tools = self.provider_visible_mcp_tools().await;
        let tools = self.tools.read().await;
        tools.definitions_for_agent(&agent_str, &self.config, &mcp_tools)
    }

    async fn transform_system(
        &self,
        baseline: String,
        model: &str,
        provider: &str,
        base_url: Option<&str>,
        effort: Option<&str>,
        agent: &deepseekcode_schema::agent::AgentKind,
    ) -> Result<String> {
        // Issue-6: byte-stable fast path. When hooks are empty the transform
        // is pure (baseline + plan builtin + config prompt) so we cache by
        // content hash. The key binds provider/base_url/effort as well: the
        // same model id on different gateways (or flipped thinking) must not
        // share a cached system string.
        if self.hooks.is_empty() {
            let agent_key = format!("{agent:?}").to_lowercase();
            let cfg_prompt = self
                .config
                .agents
                .get(&agent_key)
                .and_then(|a| a.system_prompt.as_deref())
                .unwrap_or("");
            let cache_key = format!(
                "{}|{}|{}|{}|{}|{}|{}",
                sha256_hex(baseline.as_bytes()),
                agent_key,
                model,
                provider,
                base_url.unwrap_or("default"),
                effort.unwrap_or("default"),
                sha256_hex(cfg_prompt.as_bytes())
            );
            if let Some(hit) = self.system_cache.lock().await.get(&cache_key) {
                return Ok(hit.clone());
            }
            let merged = merge_plan_prompt(baseline, agent, cfg_prompt);
            {
                let mut cache = self.system_cache.lock().await;
                // Bound to 128 entries: evict an arbitrary entry when full.
                if cache.len() >= 128 && !cache.contains_key(&cache_key) {
                    if let Some(k) = cache.keys().next().cloned() {
                        cache.remove(&k);
                    }
                }
                cache.insert(cache_key, merged.clone());
            }
            return Ok(merged);
        }
        let agent_key = format!("{agent:?}").to_lowercase();
        let cfg_prompt = self
            .config
            .agents
            .get(&agent_key)
            .and_then(|a| a.system_prompt.as_deref())
            .unwrap_or("");
        let merged = merge_plan_prompt(baseline, agent, cfg_prompt);
        let mut output = deepseekcode_plugin::ChatSystemTransformOutput {
            system: vec![merged],
        };
        self.hooks
            .run_chat_system_transform(
                &deepseekcode_plugin::ChatSystemTransformInput {
                    session_id: Some(self.session_id.to_string()),
                    model: model.to_string(),
                },
                &mut output,
            )
            .await?;
        Ok(output.system.join("\n\n"))
    }

    pub async fn drain(&self) -> Result<()> {
        self.interrupted.reset();
        // Register the live cancel token so policy prompts can be aborted
        // by `interrupt` instead of blocking 300s (P0-4).
        self.permissions
            .register_cancel(self.session_id, self.interrupted.clone())
            .await;
        self.in_provider_turn.store(false, Ordering::SeqCst);
        self.strip_reasoning_next_turn
            .store(false, Ordering::SeqCst);
        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %self.session_id,
            "session drain started"
        );

        self.context_registry
            .ensure_epoch(&self.context_epochs, &self.sessions, self.session_id)
            .await?;

        loop {
            if self.interrupted.is_cancelled() {
                return Err(DeepSeekCodeError::SessionInterrupted);
            }

            // Spec Epoch Rule 3: at every safe boundary promote input,
            // then ALWAYS reconcile context, then run provider turn.
            // Previously reconcile was skipped when nothing was promoted,
            // so AGENTS.md edits never merged without a new prompt.
            let _promoted = self.promote_at_safe_boundary().await?;
            self.reconcile_context().await?;

            if self.has_pending_prompts().await? && !self.context_registry.can_initialize_baseline()
            {
                tracing::warn!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    "mandatory context unavailable; prompts remain pending"
                );
                return Err(deepseekcode_types::DeepSeekCodeError::ContextUnavailable(
                    format!(
                        "add AGENTS.md to the project root (or configure instructions in \
                         {CONFIG_FILE} for {PRODUCT_NAME}), then send again"
                    ),
                ));
            }

            let had_work = self.run_turn_until_settled().await?;
            // Stop once all admitted prompts are promoted and the current turn
            // has settled (assistant text-only reply). Without this, a completed
            // analysis would immediately start another provider turn.
            if !self.has_pending_prompts().await? {
                break;
            }
            if !had_work {
                break;
            }
        }

        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %self.session_id,
            "session drain completed"
        );
        Ok(())
    }

    async fn has_pending_prompts(&self) -> Result<bool> {
        let pending = self.sessions.pending_inputs(self.session_id).await?;
        Ok(pending.iter().any(|p| !p.promoted))
    }

    async fn promote_at_safe_boundary(&self) -> Result<bool> {
        let promoted = PromptPromoter::new(
            self.session_id,
            &self.sessions,
            self.events.clone(),
            &self.in_provider_turn,
        )
        .promote_at_safe_boundary(self.context_registry.can_initialize_baseline())
        .await?;
        if promoted {
            // New user content starts a fresh loop-guard turn so
            // `A,user,A,user,A` is not mis-killed (P0-4).
            self.loop_guard_turn
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(promoted)
    }

    async fn note_tool_success(&self, fp: &str) {
        // Successful distinct outcomes reset the streak for that fingerprint.
        let mut guard = self.loop_guard_counts.lock().await;
        guard.remove(fp);
    }

    async fn promote_steer_if_any(&self) -> Result<bool> {
        PromptPromoter::new(
            self.session_id,
            &self.sessions,
            self.events.clone(),
            &self.in_provider_turn,
        )
        .promote_steer_if_any()
        .await
    }

    async fn run_turn_until_settled(&self) -> Result<bool> {
        let session = self.sessions.get(self.session_id).await?;
        let max_turns = agent_step_limit(&self.config, &session.agent);

        let mut turns = 0usize;
        let mut had_work = false;
        let mut max_steps_summary_done = false;

        let mut overflow_retried = false;

        loop {
            if self.interrupted.is_cancelled() {
                return Err(DeepSeekCodeError::SessionInterrupted);
            }

            if self.in_provider_turn.load(Ordering::SeqCst) {
                let _ = self.promote_steer_if_any().await?;
            }

            let response: Option<ProviderTurnOutcome> = 'provider_turn: loop {
                let session = self.sessions.get(self.session_id).await?;
                let mut llm_messages = loop {
                    let msgs = self.build_llm_messages().await?;
                    if msgs.is_empty() {
                        self.in_provider_turn.store(false, Ordering::SeqCst);
                        return Ok(had_work);
                    }
                    if self.maybe_auto_compact(&session, &msgs).await? {
                        continue;
                    }
                    break msgs;
                };

                turns += 1;
                let force_text_only = max_turns.is_some_and(|max| turns >= max);
                if max_turns.is_some_and(|max| turns > max) {
                    self.in_provider_turn.store(false, Ordering::SeqCst);
                    break None;
                }
                if force_text_only {
                    if max_steps_summary_done {
                        self.in_provider_turn.store(false, Ordering::SeqCst);
                        break None;
                    }
                    tracing::info!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %self.session_id,
                        turn = turns,
                        max_turns = ?max_turns,
                        "agent step limit reached; forcing text-only summary turn"
                    );
                    llm_messages.push(LlmMessage {
                        role: LlmRole::Assistant,
                        content: MAX_STEPS_PROMPT.into(),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                    max_steps_summary_done = true;
                }

                self.in_provider_turn.store(true, Ordering::SeqCst);
                had_work = true;

                match self
                    .stream_provider_turn(
                        &session,
                        llm_messages,
                        turns,
                        max_turns,
                        !overflow_retried,
                        force_text_only,
                    )
                    .await
                {
                    Ok(outcome) => {
                        if force_text_only {
                            self.in_provider_turn.store(false, Ordering::SeqCst);
                            return Ok(had_work);
                        }
                        break Some(outcome);
                    }
                    Err(DeepSeekCodeError::RetryAfterCompaction) => {
                        if overflow_retried {
                            self.in_provider_turn.store(false, Ordering::SeqCst);
                            return Err(DeepSeekCodeError::Llm(
                                "context overflow persisted after compaction retry".into(),
                            ));
                        }
                        overflow_retried = true;
                        turns -= 1;
                        continue 'provider_turn;
                    }
                    // Reasoning-shape recovery is a NEW provider turn with
                    // stripped messages, so each turn still performs exactly
                    // one llm.stream (spec: One llm.stream per provider turn).
                    Err(DeepSeekCodeError::RetryWithoutReasoning) => {
                        turns -= 1;
                        // Strip reasoning for the next turn attempt.
                        // The next iteration rebuilds messages via
                        // build_llm_messages(); force strip by clearing
                        // reasoning in a one-shot flag handled below.
                        self.strip_reasoning_next_turn.store(true, Ordering::SeqCst);
                        continue 'provider_turn;
                    }
                    Err(e) => {
                        self.in_provider_turn.store(false, Ordering::SeqCst);
                        return Err(e);
                    }
                }
            };

            let Some(outcome) = response else {
                break;
            };
            let response = outcome.response;

            if response.tool_calls.is_empty() {
                self.in_provider_turn.store(false, Ordering::SeqCst);
                break;
            }

            for tool_call in response.tool_calls {
                if self.interrupted.is_cancelled() {
                    self.in_provider_turn.store(false, Ordering::SeqCst);
                    return Err(DeepSeekCodeError::SessionInterrupted);
                }
                let session = self.sessions.get(self.session_id).await?;
                if let Err(e) = self
                    .execute_tool(&session, tool_call, outcome.assistant_message_id)
                    .await
                {
                    self.in_provider_turn.store(false, Ordering::SeqCst);
                    return Err(e);
                }
            }
        }

        Ok(had_work)
    }

    async fn compact_on_overflow(
        &self,
        session: &deepseekcode_schema::session::Session,
        system: &str,
        llm_messages: &[LlmMessage],
    ) -> Result<()> {
        let agent = format!("{:?}", session.agent).to_lowercase();
        let mcp_tools = self.provider_visible_mcp_tools().await;
        let tools = {
            let registry = self.tools.read().await;
            registry.definitions_for_agent(&agent, &self.config, &mcp_tools)
        };
        let tools_json = serde_json::to_value(&tools).unwrap_or_else(|_| json!([]));
        let ctx = CompactionContext {
            session_id: self.session_id,
            session,
            config: &self.config,
            sessions: &self.sessions,
            events: self.events.as_ref(),
            llm: &self.llm,
            providers: self.providers.as_ref(),
            registry: &self.context_registry,
            epochs: &self.context_epochs,
            system,
            messages: llm_messages,
            tools_json,
            hooks: Some(self.hooks.as_ref()),
            failure_slot: Some(&self.compaction_failure),
        };
        if !CompactionService::compact_after_overflow(&ctx).await? {
            return Err(DeepSeekCodeError::Llm(
                "context overflow compaction produced no checkpoint".into(),
            ));
        }
        Ok(())
    }

    async fn maybe_auto_compact(
        &self,
        session: &deepseekcode_schema::session::Session,
        llm_messages: &[LlmMessage],
    ) -> Result<bool> {
        let (baseline, _) = self.prepare_context_epoch().await?;
        let agent = format!("{:?}", session.agent).to_lowercase();
        let mcp_tools = self.provider_visible_mcp_tools().await;
        let tools = {
            let registry = self.tools.read().await;
            registry.definitions_for_agent(&agent, &self.config, &mcp_tools)
        };
        let tools_json = serde_json::to_value(&tools).unwrap_or_else(|_| json!([]));
        let ctx = CompactionContext {
            session_id: self.session_id,
            session,
            config: &self.config,
            sessions: &self.sessions,
            events: self.events.as_ref(),
            llm: &self.llm,
            providers: self.providers.as_ref(),
            registry: &self.context_registry,
            epochs: &self.context_epochs,
            system: &baseline,
            messages: llm_messages,
            tools_json,
            hooks: Some(self.hooks.as_ref()),
            failure_slot: Some(&self.compaction_failure),
        };
        CompactionService::compact_if_needed(&ctx).await
    }

    /// Manual `/compact`: one forced checkpoint built from the live context —
    /// real baseline, real projected messages, real provider-visible tools —
    /// so the stored lineage matches subsequent turns and validates on read.
    /// P0-2: never a hollow context; history is appended, never rewritten.
    pub async fn compact_manual_turn(&self) -> Result<bool> {
        // P1-3: explicit operator intent bypasses the same-view block (mirrors
        // Reasonix `trigger != manual`): the user asked for exactly one fresh
        // attempt. Fresh failures below still re-arm the record.
        self.compaction_failure.lock().await.take();
        // Rule 1 like drain: a fresh session has no epoch yet; initialize it
        // instead of failing manual `/compact` with ContextUnavailable.
        // Empty history then compacts to a no-op Ok(false), not an error.
        self.context_registry
            .ensure_epoch(&self.context_epochs, &self.sessions, self.session_id)
            .await?;
        let session = self.sessions.get(self.session_id).await?;
        let (baseline, _) = self.prepare_context_epoch().await?;
        let llm_messages = self.build_llm_messages().await?;
        let agent = format!("{:?}", session.agent).to_lowercase();
        let mcp_tools = self.provider_visible_mcp_tools().await;
        let tools = {
            let registry = self.tools.read().await;
            registry.definitions_for_agent(&agent, &self.config, &mcp_tools)
        };
        let tools_json = serde_json::to_value(&tools).unwrap_or_else(|_| json!([]));
        let ctx = CompactionContext {
            session_id: self.session_id,
            session: &session,
            config: &self.config,
            sessions: &self.sessions,
            events: self.events.as_ref(),
            llm: &self.llm,
            providers: self.providers.as_ref(),
            registry: &self.context_registry,
            epochs: &self.context_epochs,
            system: &baseline,
            messages: &llm_messages,
            tools_json,
            hooks: Some(self.hooks.as_ref()),
            failure_slot: Some(&self.compaction_failure),
        };
        CompactionService::compact_forced(&ctx).await
    }

    pub(super) fn tool_output_limits(&self) -> ToolOutputLimits {
        let cfg = self.config.tool_output.as_ref();
        // Clamp to the 32KiB prefix budget even if the user configured a
        // larger limit: provider-visible history must stay bounded for the
        // DeepSeek prefix cache, while the full output remains on disk.
        ToolOutputLimits {
            max_lines: cfg
                .and_then(|c| c.max_lines)
                .map(|v| (v as usize).min(400))
                .unwrap_or(deepseekcode_storage::DEFAULT_MAX_LINES),
            max_bytes: cfg
                .and_then(|c| c.max_bytes)
                .map(|v| (v as usize).min(32 * 1024))
                .unwrap_or(deepseekcode_storage::DEFAULT_MAX_BYTES),
        }
    }

    async fn build_llm_messages(&self) -> Result<Vec<LlmMessage>> {
        let (_baseline, baseline_seq) = self.prepare_context_epoch().await?;
        let history = EventProjector::project_messages_for_runner(
            &self.sessions,
            self.events.as_ref(),
            self.session_id,
            baseline_seq,
        )
        .await?;
        let mut messages = Vec::new();
        let mut i = 0usize;

        while i < history.len() {
            let msg = &history[i];
            match msg.role {
                MessageRole::System => {
                    let content = message_text(msg);
                    if !content.is_empty() {
                        messages.push(LlmMessage {
                            role: LlmRole::User,
                            content: format!("[System update]: {content}"),
                            tool_calls: Vec::new(),
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                    }
                    i += 1;
                }
                MessageRole::User => {
                    let content = message_text(msg);
                    if !content.is_empty() {
                        messages.push(LlmMessage {
                            role: LlmRole::User,
                            content,
                            tool_calls: Vec::new(),
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                    }
                    i += 1;
                }
                MessageRole::Assistant => {
                    // Issue-11: merging consecutive assistant rows must not drop
                    // any content/reasoning or the next-turn replay diverges
                    // from what was sent (prefix miss + quality loss).
                    // Concatenate text with newlines, concatenate reasoning,
                    // and extend tool_calls — nothing is discarded.
                    let mut contents = vec![message_text(msg)];
                    let mut reasonings = vec![message_reasoning(msg)];
                    let mut tool_calls = message_tool_calls(msg);
                    let mut j = i + 1;
                    while j < history.len() && matches!(history[j].role, MessageRole::Assistant) {
                        let next_calls = message_tool_calls(&history[j]);
                        if next_calls.is_empty() {
                            break;
                        }
                        let next_text = message_text(&history[j]);
                        if !next_text.is_empty() {
                            contents.push(next_text);
                        }
                        if let Some(r) = message_reasoning(&history[j]) {
                            if !r.is_empty() {
                                reasonings.push(Some(r));
                            }
                        }
                        tool_calls.extend(next_calls);
                        j += 1;
                    }
                    let content = contents
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let flat_reason: Vec<String> = reasonings
                        .into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .collect();
                    let reasoning = if flat_reason.is_empty() {
                        None
                    } else {
                        Some(flat_reason.join("\n"))
                    };
                    messages.push(LlmMessage {
                        role: LlmRole::Assistant,
                        content,
                        tool_calls,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: reasoning,
                    });
                    i = j;
                }
                MessageRole::Compaction => {
                    if let Some((summary, recent)) = parse_compaction_message(msg) {
                        messages.push(LlmMessage {
                            role: LlmRole::User,
                            content: compaction_checkpoint_content(&summary, &recent),
                            tool_calls: Vec::new(),
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                    }
                    i += 1;
                }
                MessageRole::Tool => {
                    for part in &msg.parts {
                        if !matches!(part.kind, PartKind::ToolResult) {
                            continue;
                        }
                        let Some(tool_call_id) = part.tool_call_id.as_ref() else {
                            tracing::warn!(
                                target: deepseekcode_telemetry::SESSION,
                                session_id = %self.session_id,
                                tool = ?part.tool_name,
                                "tool result missing tool_call_id"
                            );
                            continue;
                        };
                        let content = part
                            .tool_output
                            .as_ref()
                            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| v.to_string()))
                            .unwrap_or_default();
                        if content.is_empty() {
                            messages.push(LlmMessage {
                                role: LlmRole::Tool,
                                content: "{}".into(),
                                tool_call_id: Some(tool_call_id.clone()),
                                name: part.tool_name.clone(),
                                tool_calls: Vec::new(),
                                reasoning_content: None,
                            });
                            continue;
                        }
                        messages.push(LlmMessage {
                            role: LlmRole::Tool,
                            content: truncate_tool_content_for_prefix(&content),
                            tool_call_id: Some(tool_call_id.clone()),
                            name: part.tool_name.clone(),
                            tool_calls: Vec::new(),
                            reasoning_content: None,
                        });
                    }
                    i += 1;
                }
            }
        }

        repair_llm_tool_chain(&mut messages);

        let assistant_ids: Vec<String> = messages
            .iter()
            .filter(|m| m.role == LlmRole::Assistant)
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        let tool_ids: Vec<String> = messages
            .iter()
            .filter(|m| m.role == LlmRole::Tool)
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        for id in &assistant_ids {
            if !tool_ids.contains(id) {
                tracing::error!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    tool_call_id = %id,
                    assistant_tool_call_ids = ?assistant_ids,
                    tool_result_ids = ?tool_ids,
                    "tool result still missing after llm chain repair"
                );
            }
        }
        if messages.len() > 30 {
            tracing::debug!(
                target: deepseekcode_telemetry::SESSION,
                session_id = %self.session_id,
                message_count = messages.len(),
                "llm context size"
            );
        }
        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %self.session_id,
            assistant_tool_call_ids = ?assistant_ids,
            tool_result_ids = ?tool_ids,
            message_count = messages.len(),
            "built llm messages"
        );

        Ok(messages)
    }

    #[doc(hidden)]
    pub async fn test_build_llm_messages(&self) -> Result<Vec<LlmMessage>> {
        self.build_llm_messages().await
    }

    async fn stream_provider_turn(
        &self,
        session: &deepseekcode_schema::session::Session,
        messages: Vec<LlmMessage>,
        turn: usize,
        max_turns: Option<usize>,
        allow_overflow_recovery: bool,
        force_text_only: bool,
    ) -> Result<ProviderTurnOutcome> {
        let (baseline, _) = self.prepare_context_epoch().await?;

        let model = session
            .model_id
            .clone()
            .or_else(|| self.config.model.as_ref().and_then(|m| m.id.clone()))
            .unwrap_or_else(|| "gpt-4o-mini".into());
        let provider = session
            .provider_id
            .clone()
            .or_else(|| self.config.model.as_ref().and_then(|m| m.provider.clone()))
            .unwrap_or_else(|| "openai".into());

        let api_key = self.providers.resolve_api_key(&provider).await;
        let base_url = self.providers.get_base_url(&provider).await;

        // Issue-2: effort is real and part of the lineage. Config
        // `model.effort` flows through; changing it starts a new prefix.
        // (DeepSeek detection lives in `LlmRequest::is_deepseek` / protocol.)
        // Resolved before `transform_system` so the system cache key binds it.
        let effort = self
            .config
            .model
            .as_ref()
            .and_then(|m| m.effort.clone())
            .filter(|s| !s.trim().is_empty());

        let baseline = self
            .transform_system(
                baseline,
                &model,
                &provider,
                base_url.as_deref(),
                effort.as_deref(),
                &session.agent,
            )
            .await?;

        let tools = if force_text_only {
            Vec::new()
        } else {
            self.tool_definitions(&session.agent).await
        };
        // Issue-7: the max-steps fallback is a text-only summary turn with
        // empty `tools`, so the protocol layer would send `max_tokens` if
        // set. Cap it at 2048: the turn must summarize, never balloon into
        // a full continuation that poisons the next prefix.
        let max_output_tokens = if force_text_only { Some(2048) } else { None };
        let request = LlmRequest {
            provider: provider.clone(),
            model: model.clone(),
            system: baseline.clone(),
            messages,
            tools,
            base_url,
            // DeepSeek official main turns omit so the server uses its 384K
            // ceiling. Non-DeepSeek leaves the default path untouched.
            // (`force_text_only` sets an explicit cap above.)
            effort,
            max_output_tokens,
        };
        // Issues 7/P0-1: tools_hash + lineage for prefix_changed attribution.
        // Single hash source shared with the compaction write path: sha256 of
        // the canonical provider-visible `tools` JSON (name-sorted,
        // canonicalized schemas). Hashing the final array — not the registry
        // internals — guarantees request path and compaction path produce
        // byte-identical lineage strings for the same contract, so validation
        // never false-positives. MCP names reach the array only when directly
        // exposed (`provider_visible_mcp_tools`); otherwise the proxy keeps
        // the contract (and hash) stable across MCP churn.
        let tools_hash = deepseekcode_llm::request::tools_hash_for_json(
            &serde_json::to_value(&request.tools).unwrap_or_else(|_| json!([])),
        );
        let lineage = request.lineage_key(&tools_hash);
        // Issue-1 (read path): validate the stored checkpoint against the
        // current lineage. Mismatch never deletes history; it only attributes
        // the coming cold miss instead of letting it go unexplained.
        let prefix_changed: Option<&'static str> = {
            match self.sessions.list_messages(self.session_id).await {
                Ok(history) => {
                    let (valid, reason) =
                        crate::compaction::validate_compaction_lineage(&history, &lineage);
                    if !valid && reason != "no-checkpoint" {
                        tracing::warn!(
                            target: deepseekcode_telemetry::SESSION,
                            session_id = %self.session_id,
                            reason = reason,
                            current_lineage = %lineage,
                            "prefix_changed: lineage — stored checkpoint not from this lineage",
                        );
                        Some(reason)
                    } else if valid {
                        // Lineage matches: recompute `covered_hash` over the
                        // pre-checkpoint canonical messages with the same
                        // tail budget the write path used (P1-3). A mismatch
                        // means the covered head diverged (tampering/partial
                        // coverage) and the coming turn cold-misses.
                        // Unknown window: cannot recompute, so skip coverage
                        // (lineage already matched) rather than false-positive.
                        match crate::compaction::resolve_context_window(
                            self.providers.as_ref(),
                            session,
                        ) {
                            None => None,
                            Some(window) => {
                                let keep =
                                    crate::compaction::keep_tokens_for_config(&self.config, window);
                                let (covered_valid, covered_reason) =
                                    verify_checkpoint_coverage(&history, keep);
                                if covered_valid {
                                    None
                                } else {
                                    tracing::warn!(
                                        target: deepseekcode_telemetry::SESSION,
                                        session_id = %self.session_id,
                                        reason = covered_reason,
                                        current_lineage = %lineage,
                                        "prefix_changed: covered — checkpoint coverage diverged",
                                    );
                                    Some(covered_reason)
                                }
                            }
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        };

        tracing::debug!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %self.session_id,
            provider = %provider,
            model = %model,
            message_count = request.messages.len(),
            "provider turn starting"
        );

        self.events
            .append(
                EventKind::ProviderTurnStarted,
                Some(self.session_id),
                json!({
                    "provider": provider,
                    "model": model,
                    "message_count": request.messages.len(),
                    "turn": turn,
                    "max_turns": max_turns.unwrap_or(0),
                    "force_text_only": force_text_only,
                    "effort": request.effort,
                    "tools_hash": tools_hash,
                    "lineage": lineage,
                    "prefix_changed": prefix_changed,
                }),
            )
            .await?;

        let assistant_id = MessageId::new();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: BTreeMap<usize, LlmToolCall> = BTreeMap::new();
        let mut tool_call_args: BTreeMap<usize, String> = BTreeMap::new();
        let mut assistant_started = false;
        let mut overflow_error: Option<String> = None;
        // Provider-reported usage (DeepSeek `prompt_cache_hit/miss_tokens`
        // via `stream_options.include_usage`). Falls back to estimates below.
        let mut provider_cache_hit: Option<u32> = None;
        let mut provider_cache_miss: Option<u32> = None;
        let mut provider_cache_write: Option<u32> = None;
        let mut provider_input: Option<u32> = None;
        let mut provider_output: Option<u32> = None;
        let mut provider_reasoning: Option<u32> = None;

        // Spec: exactly one llm.stream per provider turn. Reasoning-shape
        // recovery is deferred to the NEXT turn (RetryWithoutReasoning) so
        // this function never issues a second stream in the same turn.
        // A one-shot strip flag from the previous recovery is honored here.
        let mut request = request;
        if self.strip_reasoning_next_turn.swap(false, Ordering::SeqCst) {
            for m in &mut request.messages {
                m.reasoning_content = None;
            }
        }
        let mut stream = {
            let llm = self.llm.lock().await;
            match llm.stream(&request, api_key.clone()).await {
                Ok(s) => s,
                Err(e) => {
                    let text = e.to_string();
                    let had_reasoning = request
                        .messages
                        .iter()
                        .any(|m| m.reasoning_content.as_ref().is_some_and(|s| !s.is_empty()));
                    if !should_retry_without_reasoning(&text, had_reasoning) {
                        // Open-phase overflow (HTTP 400/413 before streaming)
                        // must enter compaction recovery, not bubble as Llm.
                        if deepseekcode_llm::is_context_overflow(&text) && allow_overflow_recovery {
                            self.compact_on_overflow(session, &baseline, &request.messages)
                                .await?;
                            return Err(DeepSeekCodeError::RetryAfterCompaction);
                        }
                        return Err(e);
                    }
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        session_id = %self.session_id,
                        provider = %request.provider,
                        model = %request.model,
                        error = %text,
                        "provider rejected reasoning replay shape; deferring stripped retry to next turn"
                    );
                    return Err(DeepSeekCodeError::RetryWithoutReasoning);
                }
            }
        };

        let stream_timeout = std::time::Duration::from_secs(90);
        loop {
            let next = tokio::time::timeout(stream_timeout, stream.next()).await;
            // Steer delivery must promote DURING the provider turn, not only
            // at the next loop head. Opportunistically drain steer inbox while
            // streaming so a concurrent steer lands before the turn settles.
            // Best-effort: promotion failures never abort the stream.
            let _ = self.promote_steer_if_any().await;
            let event = match next {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    tracing::error!(
                        target: deepseekcode_telemetry::LLM,
                        session_id = %self.session_id,
                        provider = %request.provider,
                        model = %request.model,
                        "llm stream idle timeout"
                    );
                    return Err(DeepSeekCodeError::Llm(
                        "provider stream timed out waiting for data".into(),
                    ));
                }
            };
            if self.interrupted.is_cancelled() {
                if assistant_started {
                    // Persist only the streamed text/reasoning. Tool calls are
                    // deliberately NOT persisted: none of them have started
                    // executing at this point, and storing them would leave a
                    // dangling tool_use without a tool result (which breaks
                    // strict tool-chain validation on resume).
                    self.persist_interrupted_assistant_turn(assistant_id, &content, &reasoning)
                        .await?;
                }
                return Err(DeepSeekCodeError::SessionInterrupted);
            }
            match event? {
                LlmEvent::ReasoningDelta(delta) => {
                    assistant_started = true;
                    reasoning.push_str(&delta);
                    self.events
                        .append(
                            EventKind::MessageDelta,
                            Some(self.session_id),
                            json!({
                                "message_id": assistant_id.to_string(),
                                "delta": delta,
                                "part_kind": "reasoning",
                            }),
                        )
                        .await?;
                }
                LlmEvent::TextDelta(delta) => {
                    assistant_started = true;
                    content.push_str(&delta);
                    self.events
                        .append(
                            EventKind::MessageDelta,
                            Some(self.session_id),
                            json!({
                                "message_id": assistant_id.to_string(),
                                "delta": delta,
                            }),
                        )
                        .await?;
                }
                LlmEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments,
                } => {
                    assistant_started = true;
                    let entry = tool_calls.entry(index).or_insert_with(|| LlmToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: json!({}),
                    });
                    if !id.is_empty() {
                        entry.id = id;
                    }
                    if !name.is_empty() {
                        entry.name = name;
                    }
                    if !arguments.is_empty() {
                        tool_call_args
                            .entry(index)
                            .or_default()
                            .push_str(&arguments);
                    }
                }
                LlmEvent::Usage(u) => {
                    // Prefer provider-reported accounting (DeepSeek cache
                    // hit/miss/write) over local estimates. Keep the last one.
                    if let Some(hit) = u.cache_hit.or(u.cache_tokens) {
                        provider_cache_hit = Some(hit);
                    }
                    if let Some(miss) = u.cache_miss {
                        provider_cache_miss = Some(miss);
                    }
                    // Issue-3: `write` is a subset of miss billed at a
                    // premium; dropping it blinds cost attribution.
                    if let Some(w) = u.cache_write {
                        provider_cache_write = Some(w);
                    }
                    if let Some(input) = u.input_tokens {
                        provider_input = Some(input);
                    }
                    if let Some(output) = u.output_tokens {
                        provider_output = Some(output);
                    }
                    if let Some(r) = u.reasoning_tokens {
                        provider_reasoning = Some(r);
                    }
                }
                LlmEvent::Completed => break,
                LlmEvent::Error(e) => {
                    if !assistant_started && is_context_overflow(&e) {
                        overflow_error = Some(e);
                        break;
                    }
                    return Err(DeepSeekCodeError::Llm(e));
                }
            }
        }

        if let Some(err) = overflow_error {
            if !assistant_started && allow_overflow_recovery {
                tracing::warn!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    error = %err,
                    "context overflow before assistant output; compacting and retrying"
                );
                self.compact_on_overflow(session, &baseline, &request.messages)
                    .await?;
                return Err(DeepSeekCodeError::RetryAfterCompaction);
            }
            return Err(DeepSeekCodeError::Llm(err));
        }

        let calls: Vec<LlmToolCall> = tool_calls
            .into_iter()
            .map(|(index, mut call)| {
                if call.id.is_empty() {
                    call.id = ulid::Ulid::new().to_string();
                }
                if let Some(raw) = tool_call_args.get(&index) {
                    if !raw.is_empty() {
                        call.arguments = deepseekcode_types::parse_tool_arguments(raw);
                    }
                }
                call.arguments = deepseekcode_types::normalize_tool_arguments(&call.arguments);
                call
            })
            .collect();

        self.sessions
            .insert_assistant_turn(
                assistant_id,
                self.session_id,
                if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning.as_str())
                },
                &content,
            )
            .await?;

        let tool_part_offset = if reasoning.is_empty() {
            if content.is_empty() {
                0
            } else {
                1
            }
        } else if content.is_empty() {
            1
        } else {
            2
        };

        for (idx, call) in calls.iter().enumerate() {
            let tool_call_id = provider_tool_call_id(call);
            tracing::info!(
                target: deepseekcode_telemetry::SESSION,
                session_id = %self.session_id,
                tool = %call.name,
                tool_call_id = %tool_call_id,
                part_index = idx,
                arguments = %call.arguments,
                "persisting tool call"
            );
            self.sessions
                .append_tool_call(
                    assistant_id,
                    tool_part_offset + idx as i64,
                    &tool_call_id,
                    &call.name,
                    &call.arguments,
                )
                .await?;
        }

        // Issue-2: unified estimator — system + content + reasoning +
        // tool_calls + tools_json. The old hand-rolled sum omitted
        // tool_calls/tools and systematically underestimated input, which
        // then polluted derived `miss` and the 90% guard denominator.
        let tools_json = serde_json::to_value(&request.tools).unwrap_or_else(|_| json!([]));
        let estimated_input: u32 = crate::token::estimate_llm_request(
            &baseline,
            &request.messages,
            &tools_json,
            Some(model.as_str()),
        );
        // P1-4: learn the tokenizer ratio from billed provider tokens only —
        // never from estimates — so future thresholds track the real
        // tokenizer instead of drifting on `chars/3`.
        if let Some(measured) = provider_input {
            crate::token::set_calibration_from_usage(
                model.as_str(),
                crate::token::request_chars(&baseline, &request.messages, &tools_json),
                measured,
            );
        }
        // Prefer provider-reported accounting so cache hit-rate receipts and
        // guards reflect billed tokens, not estimates.
        let input_tokens = provider_input.unwrap_or(estimated_input);
        let output_tokens = provider_output.unwrap_or_else(|| estimate_message_tokens(&content));
        let reasoning_tokens =
            provider_reasoning.unwrap_or_else(|| estimate_message_tokens(&reasoning));
        let cache_hit = provider_cache_hit;
        let cache_miss = provider_cache_miss.or_else(|| match (provider_input, cache_hit) {
            (Some(input), Some(hit)) => Some(input.saturating_sub(hit)),
            _ => None,
        });
        let cache_write = provider_cache_write;

        self.sessions
            .update_message_tokens_with_cache(
                assistant_id,
                Some(input_tokens as i64),
                Some(output_tokens as i64),
                Some(reasoning_tokens as i64),
                cache_hit.map(|v| v as i64),
            )
            .await?;

        let usage_for_receipt = deepseekcode_llm::TokenUsage {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            reasoning_tokens: Some(reasoning_tokens),
            cache_tokens: cache_hit,
            cache_hit,
            cache_miss,
            cache_write,
        };
        self.events
            .append(
                EventKind::MessageCompleted,
                Some(self.session_id),
                json!({
                    "message_id": assistant_id.to_string(),
                    "content": content,
                    "reasoning": if reasoning.is_empty() { None } else { Some(&reasoning) },
                    "has_tool_calls": !calls.is_empty(),
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "reasoning_tokens": reasoning_tokens,
                    "cache_hit": cache_hit,
                    "cache_miss": cache_miss,
                    "cache_write": cache_write,
                    "cache_receipt": usage_for_receipt.receipt_fragment(),
                    "cache_hit_rate": usage_for_receipt.hit_rate(),
                }),
            )
            .await?;

        Ok(ProviderTurnOutcome {
            response: LlmResponse {
                content,
                tool_calls: calls,
                usage: Some(usage_for_receipt),
                reasoning_content: if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning)
                },
            },
            assistant_message_id: assistant_id,
        })
    }

    async fn persist_interrupted_assistant_turn(
        &self,
        assistant_id: MessageId,
        content: &str,
        reasoning: &str,
    ) -> Result<()> {
        self.sessions
            .insert_assistant_turn(
                assistant_id,
                self.session_id,
                if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning)
                },
                content,
            )
            .await?;

        self.events
            .append(
                EventKind::MessageCompleted,
                Some(self.session_id),
                json!({
                    "message_id": assistant_id.to_string(),
                    "content": content,
                    "reasoning": if reasoning.is_empty() { None } else { Some(reasoning) },
                    "has_tool_calls": false,
                    "interrupted": true,
                }),
            )
            .await?;
        Ok(())
    }
}
