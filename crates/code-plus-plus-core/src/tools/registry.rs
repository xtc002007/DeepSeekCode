use async_trait::async_trait;
use code_plus_plus_schema::tool::{ToolContext, ToolDefinition, ToolFailure, ToolResultEnvelope};
use code_plus_plus_types::{MessageId, Result, SessionId, ToolCallId};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use super::display::{build_tool_result, preview_metadata_from_input, preview_title_from_input};
use super::services::{with_services_async, with_session_id_async, ToolServices};
use crate::mcp::McpConnectionPool;
use crate::skills::{SkillCatalog, SkillTool};
use crate::snapshot::FileSnapshotStore;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure>;
}

struct ToolAlias {
    name: String,
    inner: Box<dyn Tool>,
}

#[async_trait]
impl Tool for ToolAlias {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        self.inner.execute(input, ctx).await
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    working_directory: String,
    snapshot_store: Option<Arc<FileSnapshotStore>>,
    services: ToolServices,
    /// Issue-7: when false (default) MCP stays behind the stable
    /// `use_capability` proxy; true opts into direct `mcp_*` tools at the
    /// cost of a prefix miss on every MCP change.
    mcp_direct: bool,
    /// Product logging: when true, `tool finished` includes a redacted
    /// output preview (first 500 chars). Default false for privacy; only
    /// `output_bytes` is logged otherwise. Wired from
    /// `logging.audit.include_tool_output`.
    audit_include_tool_output: bool,
}

impl ToolRegistry {
    pub fn new(
        working_directory: String,
        config: &code_plus_plus_config::CodePlusPlusConfig,
    ) -> Self {
        Self::with_services(working_directory, config, ToolServices::default())
    }

    pub fn with_services(
        working_directory: String,
        config: &code_plus_plus_config::CodePlusPlusConfig,
        services: ToolServices,
    ) -> Self {
        let shell_env = crate::shell_env::capture_shell_env(config.shell.as_deref());
        Self::with_services_and_shell_env(working_directory, config, services, shell_env)
    }

    pub fn with_services_and_shell_env(
        working_directory: String,
        config: &code_plus_plus_config::CodePlusPlusConfig,
        services: ToolServices,
        shell_env: std::collections::HashMap<String, String>,
    ) -> Self {
        let mcp_direct = config
            .mcp
            .as_ref()
            .and_then(|m| m.direct_tools)
            .unwrap_or(false);
        let audit_include_tool_output = config.logging.audit.include_tool_output;
        let mut registry = Self {
            tools: HashMap::new(),
            working_directory,
            snapshot_store: None,
            services,
            mcp_direct,
            audit_include_tool_output,
        };
        registry.register_builtin(config, shell_env);
        registry
    }

    pub fn set_services(&mut self, services: ToolServices) {
        self.services = services;
    }

    /// Hot-reload the audit preview flag without rebuilding the registry.
    /// Called on `reload_catalog`/`reload_materialization` so a config change
    /// to `logging.audit.include_tool_output` takes effect immediately.
    pub fn set_audit_include_tool_output(&mut self, enabled: bool) {
        if self.audit_include_tool_output != enabled {
            tracing::info!(
                target: code_plus_plus_telemetry::APP,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                enabled = enabled,
                "tool audit preview flag reloaded"
            );
        }
        self.audit_include_tool_output = enabled;
    }

    /// Whether MCP tools are exposed directly (`mcp_*`). When false (default)
    /// the stable `use_capability` proxy is the only MCP surface, so callers
    /// must NOT fold live MCP catalog names into the lineage hash — otherwise
    /// every MCP connect/disconnect false-positives `prefix_changed`.
    pub fn mcp_direct(&self) -> bool {
        self.mcp_direct
    }

    pub fn with_snapshot_store(mut self, store: Arc<FileSnapshotStore>) -> Self {
        self.snapshot_store = Some(store);
        self
    }

    fn register_builtin(
        &mut self,
        config: &code_plus_plus_config::CodePlusPlusConfig,
        shell_env: std::collections::HashMap<String, String>,
    ) {
        use super::builtin::*;
        use super::list::ListTool;
        use super::plan_complete::PlanCompleteTool;
        use super::question::QuestionTool;
        use super::task::TaskTool;
        use super::websearch::WebsearchTool;
        self.register(Box::new(ReadTool));
        self.register(Box::new(WriteTool));
        self.register(Box::new(EditTool));
        self.register(Box::new(GrepTool));
        self.register(Box::new(GlobTool));
        self.register(Box::new(ListTool));
        // Agent-visible PTYs (`CODE_PLUS_PLUS_BASH_PTY=1`, set by the desktop):
        // the terminal tab IS the execution site. Everywhere else (CLI,
        // server, tests) keeps pipes, preserving stdout/stderr separation.
        let agent_pty = super::agent_pty::agent_pty_enabled();
        self.register(Box::new(
            BashTool::new(config.shell.clone(), shell_env.clone()).with_pty(agent_pty),
        ));
        if cfg!(windows) {
            self.register(Box::new(
                PowershellTool::new(config.shell.clone(), shell_env.clone())
                    .with_pty(agent_pty),
            ));
        }
        self.register(Box::new(ApplyPatchTool));
        self.register(Box::new(TodoWriteTool));
        self.register(Box::new(TodoReadTool));
        self.register(Box::new(ToolAlias {
            name: "todo_write".into(),
            inner: Box::new(TodoWriteTool),
        }));
        self.register(Box::new(WebfetchTool));
        self.register(Box::new(WebsearchTool::new(config)));
        self.register(Box::new(QuestionTool));
        self.register(Box::new(PlanCompleteTool));
        self.register(Box::new(ToolAlias {
            name: "plan.complete".into(),
            inner: Box::new(PlanCompleteTool),
        }));
        self.register(Box::new(TaskTool));
        // Issue-10: stable paged recovery for truncated outputs. Frozen
        // schema so it never breaks the prefix.
        self.register(Box::new(super::tool_result_page::ToolResultPageTool));
        let skills = SkillCatalog::discover(
            &config.skills,
            &self.working_directory,
            config.skills_index_url.as_deref(),
        );
        self.register(Box::new(SkillTool::new(skills)));
        self.register(Box::new(LspTool::new(config.shell.as_deref())));
    }

    pub fn register_extensions(&mut self, mcp: &Arc<McpConnectionPool>) {
        // Issue-7: always register the frozen proxy; direct tools only on
        // explicit opt-in. Proxy keeps `tools` byte-identical across MCP churn.
        self.register(Box::new(super::use_capability::UseCapabilityTool::new(
            mcp.clone(),
        )));
        if self.mcp_direct {
            for tool in mcp.bridged_tools() {
                if tool.name() != "use_capability" {
                    self.register(tool);
                }
            }
        }
    }

    /// Alias for [`register_extensions`].
    pub fn register_mcp_tools(&mut self, mcp: &Arc<McpConnectionPool>) {
        self.register_extensions(mcp);
    }

    /// Remove all MCP-bridged tools for configured servers and re-register from the pool.
    pub fn refresh_mcp_tools(&mut self, mcp: &Arc<McpConnectionPool>) {
        let prefixes: Vec<String> = mcp
            .status()
            .into_iter()
            .map(|s| format!("mcp_{}_", s.name))
            .collect();
        self.tools
            .retain(|name, _| !prefixes.iter().any(|prefix| name.starts_with(prefix)));
        // Keep the proxy stable across refreshes; only re-add direct tools
        // when explicitly opted in (each such change is one expected miss).
        if !self.tools.contains_key("use_capability") {
            self.register(Box::new(super::use_capability::UseCapabilityTool::new(
                mcp.clone(),
            )));
        }
        if self.mcp_direct {
            for tool in mcp.bridged_tools() {
                if tool.name() != "use_capability" {
                    self.register(tool);
                }
            }
            tracing::info!(
                target: code_plus_plus_telemetry::SESSION,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                "mcp direct tools refreshed; expect one DeepSeek prefix miss"
            );
        }
    }

    /// Re-discover skills from config and replace the builtin `skill` tool.
    pub fn refresh_skill_tool(&mut self, config: &code_plus_plus_config::CodePlusPlusConfig) {
        self.tools.retain(|name, _| name != "skill");
        let skills = SkillCatalog::discover(
            &config.skills,
            &self.working_directory,
            config.skills_index_url.as_deref(),
        );
        self.register(Box::new(SkillTool::new(skills)));
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        // Cache-stable order: HashMap iteration is random and would break the
        // DeepSeek automatic prefix cache on every turn. Same LLM-safety
        // filter as `definitions_for_agent` so invalid names (e.g. aliases
        // containing `.`) can never leak into a provider request.
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|t| {
                if is_llm_safe_tool_name(t.name()) {
                    true
                } else {
                    tracing::warn!(
                        target: code_plus_plus_telemetry::SESSION,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        tool = t.name(),
                        "skipping tool with invalid LLM function name"
                    );
                    false
                }
            })
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: canonicalize_schema(&t.input_schema()),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    pub fn definitions_for_agent(
        &self,
        agent: &str,
        config: &code_plus_plus_config::CodePlusPlusConfig,
        mcp_tools: &[String],
    ) -> Vec<ToolDefinition> {
        let allowed = agent_allowed_tools_with_mcp(agent, config, mcp_tools);
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|t| allowed.iter().any(|a| a == t.name()))
            .filter(|t| {
                if is_llm_safe_tool_name(t.name()) {
                    true
                } else {
                    tracing::warn!(
                        target: code_plus_plus_telemetry::SESSION,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        tool = t.name(),
                        "skipping tool with invalid LLM function name"
                    );
                    false
                }
            })
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: canonicalize_schema(&t.input_schema()),
            })
            .collect();
        // Stable name order keeps `tools` JSON byte-identical across turns.
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        session_id: SessionId,
        assistant_message_id: MessageId,
        tool_call_id: ToolCallId,
        agent: &str,
        hooks: Option<&code_plus_plus_plugin::HookRegistry>,
        cancel: Option<Arc<crate::tools::services::CancelToken>>,
    ) -> Result<ToolResultEnvelope> {
        let tool = self.tools.get(name).ok_or_else(|| {
            tracing::warn!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %session_id,
                tool = name,
                "tool not found"
            );
            code_plus_plus_types::CodePlusPlusError::NotFound(format!("tool {name}"))
        })?;
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            tool = name,
            tool_call_id = %tool_call_id,
            agent = agent,
            input_bytes = serde_json::to_string(&input).map(|s| s.len()).unwrap_or(0),
            "tool started"
        );

        let todo_store = self.services.todo_store.as_ref().map(|store| {
            Arc::new(crate::todo::StorageTodoStoreOps(store.clone()))
                as Arc<dyn code_plus_plus_schema::tool::TodoStoreOps>
        });

        let ctx = ToolContext {
            session_id,
            agent: agent.into(),
            assistant_message_id,
            tool_call_id,
            working_directory: self.working_directory.clone(),
            extras: code_plus_plus_schema::tool::ToolContextExtras { todo_store },
        };

        if let Some(store) = &self.snapshot_store {
            if store.is_enabled() && FileSnapshotStore::is_write_tool(name) {
                let paths =
                    FileSnapshotStore::paths_from_tool_input(name, &input, &self.working_directory);
                if matches!(name, "bash" | "powershell") && paths.is_empty() {
                    tracing::info!(
                        target: code_plus_plus_telemetry::TOOL,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        session_id = %session_id,
                        tool = name,
                        "shell command has no capturable file paths; file-level revert unavailable"
                    );
                }
                for path in paths {
                    if !crate::project_context::path_within_project(&self.working_directory, &path)
                    {
                        return Err(code_plus_plus_types::CodePlusPlusError::ToolFailure(format!(
                            "path outside project directory: `{}` is not under the project root `{}`. \
                             Use a relative path inside the project.",
                            path.display(),
                            self.working_directory
                        )));
                    }
                    store
                        .capture(session_id, Some(assistant_message_id), &path)
                        .await?;
                }
            }
        }

        let mut hook_input = input.clone();
        if let Some(hooks) = hooks {
            let mut before_out = code_plus_plus_plugin::ToolExecuteBeforeOutput {
                args: hook_input.clone(),
            };
            hooks
                .run_tool_execute_before(
                    &code_plus_plus_plugin::ToolExecuteBeforeInput {
                        tool: name.to_string(),
                        session_id: session_id.to_string(),
                        call_id: tool_call_id.to_string(),
                    },
                    &mut before_out,
                )
                .await?;
            hook_input = before_out.args;
        }

        let start = std::time::Instant::now();
        let services = if let Some(cancel) = cancel.clone() {
            self.services.clone().with_cancel(cancel)
        } else {
            self.services.clone()
        };
        let cancel_flag = cancel.clone();
        let execute_fut = with_services_async(services, async {
            with_session_id_async(session_id, async {
                tool.execute(hook_input.clone(), &ctx).await
            })
            .await
        });
        let result = if let Some(cancel) = cancel_flag {
            tokio::select! {
                result = execute_fut => result,
                _ = cancel.cancelled() => {
                    Err(ToolFailure {
                        message: "tool execution interrupted".into(),
                        retryable: false,
                    })
                }
            }
        } else {
            execute_fut.await
        };
        let duration_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(mut value) => {
                let mut hook_title = None;
                let mut hook_metadata = Value::Object(Default::default());
                if matches!(name, "bash" | "powershell")
                    && FileSnapshotStore::paths_from_tool_input(
                        name,
                        &input,
                        &self.working_directory,
                    )
                    .is_empty()
                {
                    hook_metadata = json!({
                        "non_revertable": true,
                        "revert_warning": "Shell command has no capturable file paths; timeline revert will not restore workspace files."
                    });
                }
                if let Some(hooks) = hooks {
                    let mut after_out = code_plus_plus_plugin::ToolExecuteAfterOutput {
                        title: preview_title_from_input(name, &hook_input),
                        output: value.clone(),
                        metadata: hook_metadata.clone(),
                    };
                    hooks
                        .run_tool_execute_after(
                            &code_plus_plus_plugin::ToolExecuteAfterInput {
                                tool: name.to_string(),
                                session_id: session_id.to_string(),
                                call_id: tool_call_id.to_string(),
                                args: hook_input,
                            },
                            &mut after_out,
                        )
                        .await?;
                    value = after_out.output;
                    hook_title = Some(after_out.title);
                    if let (Some(base), Some(overlay)) = (
                        hook_metadata.as_object_mut(),
                        after_out.metadata.as_object(),
                    ) {
                        for (key, val) in overlay {
                            base.insert(key.clone(), val.clone());
                        }
                    }
                }
                let envelope =
                    build_tool_result(name, &input, value, duration_ms, hook_title, hook_metadata);
                let size = serde_json::to_string(&envelope.output)
                    .map(|s| s.len())
                    .unwrap_or(0);
                // `audit.include_tool_output=true` opts into a redacted
                // preview (first 500 chars) for deep troubleshooting.
                // Default false: only size/title/duration are logged to avoid
                // leaking file contents or secrets into `session.log`.
                if self.audit_include_tool_output {
                    let raw = serde_json::to_string(&envelope.output).unwrap_or_default();
                    let preview: String = raw.chars().take(500).collect();
                    tracing::info!(
                        target: code_plus_plus_telemetry::TOOL,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        session_id = %session_id,
                        tool = name,
                        tool_call_id = %tool_call_id,
                        duration_ms,
                        output_bytes = size,
                        title = %envelope.title,
                        output_preview = %code_plus_plus_telemetry::redact(&preview),
                        "tool finished"
                    );
                } else {
                    tracing::info!(
                        target: code_plus_plus_telemetry::TOOL,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        session_id = %session_id,
                        tool = name,
                        tool_call_id = %tool_call_id,
                        duration_ms,
                        output_bytes = size,
                        title = %envelope.title,
                        "tool finished"
                    );
                }
                Ok(envelope)
            }
            Err(e) => {
                tracing::warn!(
                    target: code_plus_plus_telemetry::TOOL,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    session_id = %session_id,
                    tool = name,
                    tool_call_id = %tool_call_id,
                    duration_ms,
                    error = %e.message,
                    retryable = e.retryable,
                    "tool failed"
                );
                Err(code_plus_plus_types::CodePlusPlusError::ToolFailure(
                    e.message,
                ))
            }
        }
    }
}

pub fn preview_tool_start(name: &str, input: &Value) -> (String, Value) {
    (
        preview_title_from_input(name, input),
        preview_metadata_from_input(name, input),
    )
}

/// Canonical per-agent tool allow-list. Single source of truth — `skills.rs`
/// used to carry a forked copy that drifted (missing `question`/`task`/
/// `lsp`/`use_capability`/`tool_result_page`/`powershell`); it now delegates
/// here so the two can never diverge again.
pub(crate) fn agent_allowed_tools(
    agent: &str,
    config: &code_plus_plus_config::CodePlusPlusConfig,
) -> Vec<String> {
    if let Some(agent_cfg) = config.agents.get(agent) {
        if let Some(tools) = &agent_cfg.tools {
            return tools.clone();
        }
    }
    match agent {
        // P0-8: plan mode hides write/edit/bash/apply_patch from the model
        // (spec: "denies by default"). Writes under `.code-plus-plus/plan/**`
        // remain possible via policy permit, but the model is not offered
        // the tools; `tool_result_page` (read scope) is included so large
        // plan outputs remain pageable.
        "plan" => vec![
            "read".into(),
            "grep".into(),
            "glob".into(),
            "list".into(),
            "skill".into(),
            "question".into(),
            "plan_complete".into(),
            "tool_result_page".into(),
        ],
        _ => {
            let base = vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "grep".into(),
                "glob".into(),
                "list".into(),
                "bash".into(),
                "apply_patch".into(),
                // Canonical `todowrite` only: the `todo_write` alias stays
                // registered for executing old transcripts, but exposing both
                // names would double-occupy the provider prefix.
                "todowrite".into(),
                "todoread".into(),
                "skill".into(),
                "webfetch".into(),
                "websearch".into(),
                "question".into(),
                "task".into(),
                "lsp".into(),
                // Issue-7: stable MCP proxy is part of the default contract.
                "use_capability".into(),
                // Issue-10: paged recovery never rewrites history.
                "tool_result_page".into(),
            ];
            #[cfg(windows)]
            {
                let mut tools = base;
                tools.push("powershell".into());
                tools
            }
            #[cfg(not(windows))]
            {
                base
            }
        }
    }
}

pub fn agent_allowed_tools_with_mcp(
    agent: &str,
    config: &code_plus_plus_config::CodePlusPlusConfig,
    mcp_tools: &[String],
) -> Vec<String> {
    let mut allowed = agent_allowed_tools(agent, config);
    if agent == "plan" {
        return allowed;
    }
    for name in mcp_tools {
        if !allowed.contains(name) {
            allowed.push(name.clone());
        }
    }
    allowed
}

/// OpenAI-compatible tool names: `^[a-zA-Z0-9_-]+$`
pub fn is_llm_safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Recursively sort JSON object keys so `input_schema` serializes
/// byte-identically across turns. DeepSeek's prefix cache hashes the raw
/// `tools` JSON; unsorted keys from different construction paths would
/// otherwise cause a miss on every request.
pub fn canonicalize_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = serde_json::Map::with_capacity(map.len());
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k.clone(), canonicalize_schema(&map[k]));
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_schema).collect()),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpConnectionPool;
    use code_plus_plus_config::CodePlusPlusConfig;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn plan_agent_tool_names_are_llm_safe() {
        let allowed = agent_allowed_tools("plan", &CodePlusPlusConfig::default());
        for name in allowed {
            assert!(
                is_llm_safe_tool_name(&name),
                "plan agent tool name must be LLM-safe: {name}"
            );
        }
    }

    #[test]
    fn definitions_for_plan_agent_exclude_invalid_names() {
        let registry = ToolRegistry::new("/tmp".into(), &CodePlusPlusConfig::default());
        let defs = registry.definitions_for_agent("plan", &CodePlusPlusConfig::default(), &[]);
        assert!(defs.iter().any(|d| d.name == "plan_complete"));
        assert!(!defs.iter().any(|d| d.name == "plan.complete"));
        for def in &defs {
            assert!(
                is_llm_safe_tool_name(&def.name),
                "bad tool name: {}",
                def.name
            );
        }
    }

    #[test]
    fn refresh_mcp_tools_removes_stale_entries() {
        let mut registry = ToolRegistry::new("/tmp".into(), &CodePlusPlusConfig::default());
        let pool = Arc::new(McpConnectionPool::new(HashMap::new()));
        registry.register_extensions(&pool);
        let before = registry.definitions().len();
        registry.refresh_mcp_tools(&pool);
        let after = registry.definitions().len();
        assert_eq!(before, after);
    }

    #[test]
    fn definitions_are_sorted_for_prefix_cache() {
        let registry = ToolRegistry::new("/tmp".into(), &CodePlusPlusConfig::default());
        let defs = registry.definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "tools must serialize sorted for DeepSeek prefix"
        );
        // Twice in a row must be byte-identical (HashMap order is random).
        let again = registry.definitions();
        assert_eq!(
            serde_json::to_string(&defs).unwrap(),
            serde_json::to_string(&again).unwrap()
        );
    }

    #[test]
    fn canonicalize_sorts_object_keys() {
        let v = serde_json::json!({"z": 1, "a": {"d": 1, "b": 2}});
        let c = canonicalize_schema(&v);
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.find("\"a\"").unwrap() < s.find("\"z\"").unwrap());
        assert!(s.find("\"b\"").unwrap() < s.find("\"d\"").unwrap());
    }

    #[test]
    fn request_and_compaction_lineage_share_one_hash_source() {
        // P0-1 regression: the request path (runner) and the compaction write
        // path must hash the provider-visible contract with the SAME function
        // over the SAME JSON construction, or validation false-positives on
        // every turn (`lineage-changed`) and breaks the incremental chain.
        use code_plus_plus_llm::request::{lineage_key_for, normalize_thinking, tools_hash_for_json};
        let config = CodePlusPlusConfig::default();
        let registry = ToolRegistry::new("/tmp".into(), &config);
        // Runner path: definitions_for_agent -> JSON -> hash.
        let runner_defs = registry.definitions_for_agent("build", &config, &[]);
        let runner_json = serde_json::to_value(&runner_defs).unwrap();
        let runner_hash = tools_hash_for_json(&runner_json);
        // Compaction path: identical construction from the same registry.
        let compact_defs = registry.definitions_for_agent("build", &config, &[]);
        let compact_json = serde_json::to_value(&compact_defs).unwrap();
        let compact_hash = tools_hash_for_json(&compact_json);
        assert_eq!(
            runner_hash, compact_hash,
            "dual hash fork: request and compaction paths diverged"
        );
        let thinking = normalize_thinking("deepseek-chat", None, true);
        assert_eq!(
            lineage_key_for("deepseek", "deepseek-chat", thinking, &runner_hash),
            lineage_key_for("deepseek", "deepseek-chat", thinking, &compact_hash),
            "lineage strings must be byte-identical for the same contract"
        );
        // Schema drift must move the hash (else stale checkpoints reuse).
        assert_ne!(
            runner_hash,
            tools_hash_for_json(&serde_json::json!([{"different": true}])),
            "hash must be sensitive to contract content"
        );
    }
}
