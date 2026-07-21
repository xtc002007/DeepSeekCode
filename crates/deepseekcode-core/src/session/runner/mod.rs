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
    agent_step_limit, estimate_message_tokens, message_text, message_tool_calls,
    provider_tool_call_id, repair_llm_tool_chain,
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
    compaction_checkpoint_content, parse_compaction_message, CompactionContext, CompactionService,
};
use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::session::projector::EventProjector;
use crate::snapshot::FileSnapshotStore;
use crate::system_context::{ContextEpochStore, SystemContextRegistry};
use crate::tools::{EventToolProgressSink, ToolRegistry, ToolServices};
use deepseekcode_plugin::HookRegistry;
use deepseekcode_storage::TodoStore;

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

pub struct SessionRunner {
    session_id: SessionId,
    project_dir: String,
    config: DeepSeekCodeConfig,
    sessions: SessionStore,
    events: Arc<EventStore>,
    context_registry: SystemContextRegistry,
    context_epochs: ContextEpochStore,
    tools: Arc<Mutex<ToolRegistry>>,
    permissions: Arc<PermissionService>,
    providers: Arc<ProviderCatalog>,
    tool_output: ManagedToolOutput,
    interrupted: Arc<AtomicBool>,
    in_provider_turn: AtomicBool,
    llm: Arc<Mutex<LlmClient>>,
    mcp: Arc<McpConnectionPool>,
    hooks: Arc<HookRegistry>,
    question_tx: Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>>,
    loop_guard: Arc<Mutex<Option<(String, u32)>>>,
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
        };
        let mut tools = ToolRegistry::with_services(project_dir.clone(), &config, services)
            .with_snapshot_store(snapshot_store);
        tools.register_extensions(&mcp);
        Self {
            sessions: SessionStore::new(pool.clone()),
            context_epochs: ContextEpochStore::new(pool),
            context_registry: SystemContextRegistry::with_config(&project_dir, &config),
            tools: Arc::new(Mutex::new(tools)),
            tool_output: ManagedToolOutput::project(&project_dir),
            permissions,
            providers,
            events,
            interrupted: Arc::new(AtomicBool::new(false)),
            in_provider_turn: AtomicBool::new(false),
            llm: Arc::new(Mutex::new(LlmClient::new())),
            session_id,
            project_dir,
            config,
            mcp,
            hooks,
            question_tx,
            loop_guard: Arc::new(Mutex::new(None)),
        }
    }

    pub fn submit_question_answer(&self, answer: String) -> Result<()> {
        crate::tools::services::submit_question_answer(
            &ToolServices {
                question_tx: Some(self.question_tx.clone()),
                ..ToolServices::default()
            },
            answer,
        )
    }

    pub fn question_channel(
        &self,
    ) -> Arc<tokio::sync::Mutex<Option<crate::tools::services::QuestionResponder>>> {
        self.question_tx.clone()
    }

    pub async fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
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
        let mut tools = self.tools.lock().await;
        tools.refresh_mcp_tools(&self.mcp);
    }

    /// Re-materialize skill tool and system context after catalog changes.
    pub async fn reload_materialization(&self, config: &DeepSeekCodeConfig) {
        let mut tools = self.tools.lock().await;
        tools.refresh_skill_tool(config);
    }

    async fn tool_definitions(
        &self,
        agent: &deepseekcode_schema::agent::AgentKind,
    ) -> Vec<deepseekcode_schema::tool::ToolDefinition> {
        let agent_str = format!("{agent:?}").to_lowercase();
        let mcp_tools = self
            .mcp
            .bridged_tools()
            .into_iter()
            .map(|t| t.name().to_string())
            .collect::<Vec<_>>();
        let tools = self.tools.lock().await;
        tools.definitions_for_agent(&agent_str, &self.config, &mcp_tools)
    }

    async fn transform_system(
        &self,
        baseline: String,
        model: &str,
        agent: &deepseekcode_schema::agent::AgentKind,
    ) -> Result<String> {
        let agent_key = format!("{agent:?}").to_lowercase();
        let mut merged = baseline;
        if matches!(agent, deepseekcode_schema::agent::AgentKind::Plan) {
            let builtin = crate::agent::plan_system_prompt();
            if !builtin.is_empty() {
                merged = format!("{merged}\n\n{builtin}");
            }
        }
        if let Some(prompt) = self
            .config
            .agents
            .get(&agent_key)
            .and_then(|a| a.system_prompt.as_deref())
        {
            if !prompt.is_empty() {
                merged = format!("{merged}\n\n{prompt}");
            }
        }
        if self.hooks.is_empty() {
            return Ok(merged);
        }
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
        self.interrupted.store(false, Ordering::SeqCst);
        self.in_provider_turn.store(false, Ordering::SeqCst);
        tracing::info!(
            target: deepseekcode_telemetry::SESSION,
            session_id = %self.session_id,
            "session drain started"
        );

        self.context_registry
            .ensure_epoch(&self.context_epochs, self.events.as_ref(), self.session_id)
            .await?;

        loop {
            if self.interrupted.load(Ordering::SeqCst) {
                return Err(DeepSeekCodeError::SessionInterrupted);
            }

            let promoted = self.promote_at_safe_boundary().await?;
            if promoted {
                self.reconcile_context().await?;
            }

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
        PromptPromoter::new(
            self.session_id,
            &self.sessions,
            self.events.clone(),
            &self.in_provider_turn,
        )
        .promote_at_safe_boundary(self.context_registry.can_initialize_baseline())
        .await
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
            if self.interrupted.load(Ordering::SeqCst) {
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
        let mcp_tools = self.mcp.list_registered_tool_names();
        let tools = {
            let registry = self.tools.lock().await;
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
        let mcp_tools = self.mcp.list_registered_tool_names();
        let tools = {
            let registry = self.tools.lock().await;
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
        };
        CompactionService::compact_if_needed(&ctx).await
    }

    pub(super) fn tool_output_limits(&self) -> ToolOutputLimits {
        let cfg = self.config.tool_output.as_ref();
        ToolOutputLimits {
            max_lines: cfg
                .and_then(|c| c.max_lines)
                .map(|v| v as usize)
                .unwrap_or(deepseekcode_storage::DEFAULT_MAX_LINES),
            max_bytes: cfg
                .and_then(|c| c.max_bytes)
                .map(|v| v as usize)
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
                        });
                    }
                    i += 1;
                }
                MessageRole::Assistant => {
                    let content = message_text(msg);
                    let mut tool_calls = message_tool_calls(msg);
                    let mut j = i + 1;
                    while j < history.len() && matches!(history[j].role, MessageRole::Assistant) {
                        let next_calls = message_tool_calls(&history[j]);
                        if next_calls.is_empty() {
                            break;
                        }
                        tool_calls.extend(next_calls);
                        j += 1;
                    }
                    messages.push(LlmMessage {
                        role: LlmRole::Assistant,
                        content,
                        tool_calls,
                        tool_call_id: None,
                        name: None,
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
                            continue;
                        }
                        messages.push(LlmMessage {
                            role: LlmRole::Tool,
                            content,
                            tool_call_id: Some(tool_call_id.clone()),
                            name: part.tool_name.clone(),
                            tool_calls: Vec::new(),
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

        let baseline = self
            .transform_system(baseline, &model, &session.agent)
            .await?;

        let api_key = self.providers.resolve_api_key(&provider).await;
        let base_url = self.providers.get_base_url(&provider).await;

        let tools = if force_text_only {
            Vec::new()
        } else {
            self.tool_definitions(&session.agent).await
        };

        let request = LlmRequest {
            provider: provider.clone(),
            model: model.clone(),
            system: baseline.clone(),
            messages,
            tools,
            base_url,
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

        let mut stream = {
            let llm = self.llm.lock().await;
            llm.stream(&request, api_key).await?
        };

        let stream_timeout = std::time::Duration::from_secs(90);
        loop {
            let next = tokio::time::timeout(stream_timeout, stream.next()).await;
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
            if self.interrupted.load(Ordering::SeqCst) {
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

        let input_tokens: u32 = request
            .messages
            .iter()
            .map(|m| estimate_message_tokens(&m.content))
            .sum::<u32>()
            .saturating_add(estimate_message_tokens(&baseline));
        let output_tokens = estimate_message_tokens(&content);
        let reasoning_tokens = estimate_message_tokens(&reasoning);

        self.sessions
            .update_message_tokens(
                assistant_id,
                Some(input_tokens as i64),
                Some(output_tokens as i64),
                Some(reasoning_tokens as i64),
            )
            .await?;

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
                }),
            )
            .await?;

        Ok(ProviderTurnOutcome {
            response: LlmResponse {
                content,
                tool_calls: calls,
                usage: Some(deepseekcode_llm::TokenUsage {
                    input_tokens: Some(input_tokens),
                    output_tokens: Some(output_tokens),
                    reasoning_tokens: Some(reasoning_tokens),
                    cache_tokens: None,
                }),
            },
            assistant_message_id: assistant_id,
        })
    }
}
