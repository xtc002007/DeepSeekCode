use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolDefinition, ToolFailure, ToolResultEnvelope};
use deepseekcode_types::{MessageId, Result, SessionId, ToolCallId};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use super::display::{build_tool_result, preview_metadata_from_input, preview_title_from_input};
use super::services::{with_services_async, ToolServices};
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
}

impl ToolRegistry {
    pub fn new(
        working_directory: String,
        config: &deepseekcode_config::DeepSeekCodeConfig,
    ) -> Self {
        Self::with_services(working_directory, config, ToolServices::default())
    }

    pub fn with_services(
        working_directory: String,
        config: &deepseekcode_config::DeepSeekCodeConfig,
        services: ToolServices,
    ) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_directory,
            snapshot_store: None,
            services,
        };
        registry.register_builtin(config);
        registry
    }

    pub fn set_services(&mut self, services: ToolServices) {
        self.services = services;
    }

    pub fn with_snapshot_store(mut self, store: Arc<FileSnapshotStore>) -> Self {
        self.snapshot_store = Some(store);
        self
    }

    fn register_builtin(&mut self, config: &deepseekcode_config::DeepSeekCodeConfig) {
        use super::builtin::*;
        use super::list::ListTool;
        use super::plan_complete::PlanCompleteTool;
        use super::question::QuestionTool;
        use super::task::TaskTool;
        use super::websearch::WebsearchTool;
        let shell_env = crate::shell_env::capture_shell_env(config.shell.as_deref());
        self.register(Box::new(ReadTool));
        self.register(Box::new(WriteTool));
        self.register(Box::new(EditTool));
        self.register(Box::new(GrepTool));
        self.register(Box::new(GlobTool));
        self.register(Box::new(ListTool));
        self.register(Box::new(BashTool::new(
            config.shell.clone(),
            shell_env.clone(),
        )));
        if cfg!(windows) {
            self.register(Box::new(PowershellTool::new(
                config.shell.clone(),
                shell_env.clone(),
            )));
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
        let skills = SkillCatalog::discover(
            &config.skills,
            &self.working_directory,
            config.skills_index_url.as_deref(),
        );
        self.register(Box::new(SkillTool::new(skills)));
        self.register(Box::new(LspTool::new(config.shell.as_deref())));
    }

    pub fn register_extensions(&mut self, mcp: &Arc<McpConnectionPool>) {
        for tool in mcp.bridged_tools() {
            self.register(tool);
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
            .map(|s| format!("{}_", s.name))
            .collect();
        self.tools
            .retain(|name, _| !prefixes.iter().any(|prefix| name.starts_with(prefix)));
        for tool in mcp.bridged_tools() {
            self.register(tool);
        }
    }

    /// Re-discover skills from config and replace the builtin `skill` tool.
    pub fn refresh_skill_tool(&mut self, config: &deepseekcode_config::DeepSeekCodeConfig) {
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
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    pub fn definitions_for_agent(
        &self,
        agent: &str,
        config: &deepseekcode_config::DeepSeekCodeConfig,
        mcp_tools: &[String],
    ) -> Vec<ToolDefinition> {
        let allowed = agent_allowed_tools_with_mcp(agent, config, mcp_tools);
        self.tools
            .values()
            .filter(|t| allowed.iter().any(|a| a == t.name()))
            .filter(|t| {
                if is_llm_safe_tool_name(t.name()) {
                    true
                } else {
                    tracing::warn!(
                        target: deepseekcode_telemetry::SESSION,
                        tool = t.name(),
                        "skipping tool with invalid LLM function name"
                    );
                    false
                }
            })
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        session_id: SessionId,
        assistant_message_id: MessageId,
        tool_call_id: ToolCallId,
        agent: &str,
        hooks: Option<&deepseekcode_plugin::HookRegistry>,
    ) -> Result<ToolResultEnvelope> {
        let tool = self.tools.get(name).ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::NotFound(format!("tool {name}"))
        })?;

        let todo_store = self.services.todo_store.as_ref().map(|store| {
            Arc::new(crate::todo::StorageTodoStoreOps(store.clone()))
                as Arc<dyn deepseekcode_schema::tool::TodoStoreOps>
        });

        let ctx = ToolContext {
            session_id,
            agent: agent.into(),
            assistant_message_id,
            tool_call_id,
            working_directory: self.working_directory.clone(),
            extras: deepseekcode_schema::tool::ToolContextExtras { todo_store },
        };

        if let Some(store) = &self.snapshot_store {
            if store.is_enabled() && FileSnapshotStore::is_write_tool(name) {
                for path in
                    FileSnapshotStore::paths_from_tool_input(name, &input, &self.working_directory)
                {
                    if !crate::project_context::path_within_project(&self.working_directory, &path)
                    {
                        return Err(deepseekcode_types::DeepSeekCodeError::ToolFailure(format!(
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
            let mut before_out = deepseekcode_plugin::ToolExecuteBeforeOutput {
                args: hook_input.clone(),
            };
            hooks
                .run_tool_execute_before(
                    &deepseekcode_plugin::ToolExecuteBeforeInput {
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
        let services = self.services.clone();
        let result = with_services_async(services, async {
            tool.execute(hook_input.clone(), &ctx).await
        })
        .await;
        let duration_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(mut value) => {
                let mut hook_title = None;
                let mut hook_metadata = Value::Object(Default::default());
                if let Some(hooks) = hooks {
                    let mut after_out = deepseekcode_plugin::ToolExecuteAfterOutput {
                        title: preview_title_from_input(name, &hook_input),
                        output: value.clone(),
                        metadata: Value::Object(Default::default()),
                    };
                    hooks
                        .run_tool_execute_after(
                            &deepseekcode_plugin::ToolExecuteAfterInput {
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
                    hook_metadata = after_out.metadata;
                }
                let envelope =
                    build_tool_result(name, &input, value, duration_ms, hook_title, hook_metadata);
                let size = serde_json::to_string(&envelope.output)
                    .map(|s| s.len())
                    .unwrap_or(0);
                tracing::debug!(
                    target: deepseekcode_telemetry::TOOL,
                    session_id = %session_id,
                    tool = name,
                    duration_ms,
                    output_bytes = size,
                    title = %envelope.title,
                    "tool finished"
                );
                Ok(envelope)
            }
            Err(e) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::TOOL,
                    session_id = %session_id,
                    tool = name,
                    duration_ms,
                    error = %e.message,
                    "tool failed"
                );
                Err(deepseekcode_types::DeepSeekCodeError::ToolFailure(
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

fn agent_allowed_tools(
    agent: &str,
    config: &deepseekcode_config::DeepSeekCodeConfig,
) -> Vec<String> {
    if let Some(agent_cfg) = config.agents.get(agent) {
        if let Some(tools) = &agent_cfg.tools {
            return tools.clone();
        }
    }
    match agent {
        "plan" => vec![
            "read".into(),
            "grep".into(),
            "glob".into(),
            "list".into(),
            "write".into(),
            "edit".into(),
            "skill".into(),
            "question".into(),
            "plan_complete".into(),
        ],
        _ => {
            let mut tools = vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "grep".into(),
                "glob".into(),
                "list".into(),
                "bash".into(),
                "apply_patch".into(),
                "todowrite".into(),
                "todo_write".into(),
                "todoread".into(),
                "skill".into(),
                "webfetch".into(),
                "websearch".into(),
                "question".into(),
                "task".into(),
                "lsp".into(),
            ];
            #[cfg(windows)]
            tools.push("powershell".into());
            tools
        }
    }
}

pub fn agent_allowed_tools_with_mcp(
    agent: &str,
    config: &deepseekcode_config::DeepSeekCodeConfig,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpConnectionPool;
    use deepseekcode_config::DeepSeekCodeConfig;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn plan_agent_tool_names_are_llm_safe() {
        let allowed = agent_allowed_tools("plan", &DeepSeekCodeConfig::default());
        for name in allowed {
            assert!(
                is_llm_safe_tool_name(&name),
                "plan agent tool name must be LLM-safe: {name}"
            );
        }
    }

    #[test]
    fn definitions_for_plan_agent_exclude_invalid_names() {
        let registry = ToolRegistry::new("/tmp".into(), &DeepSeekCodeConfig::default());
        let defs = registry.definitions_for_agent("plan", &DeepSeekCodeConfig::default(), &[]);
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
        let mut registry = ToolRegistry::new("/tmp".into(), &DeepSeekCodeConfig::default());
        let pool = Arc::new(McpConnectionPool::new(HashMap::new()));
        registry.register_extensions(&pool);
        let before = registry.definitions().len();
        registry.refresh_mcp_tools(&pool);
        let after = registry.definitions().len();
        assert_eq!(before, after);
    }
}
