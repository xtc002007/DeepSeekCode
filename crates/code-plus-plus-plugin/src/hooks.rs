use async_trait::async_trait;
use code_plus_plus_types::Result;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ToolExecuteBeforeInput {
    pub tool: String,
    pub session_id: String,
    pub call_id: String,
}

#[derive(Debug, Clone)]
pub struct ToolExecuteBeforeOutput {
    pub args: Value,
}

#[derive(Debug, Clone)]
pub struct ToolExecuteAfterInput {
    pub tool: String,
    pub session_id: String,
    pub call_id: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub struct ToolExecuteAfterOutput {
    pub title: String,
    pub output: Value,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct ChatSystemTransformInput {
    pub session_id: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct ChatSystemTransformOutput {
    pub system: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionCompactingInput {
    pub session_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionCompactingOutput {
    pub context: Vec<String>,
    pub prompt: Option<String>,
}

#[async_trait]
pub trait SessionHook: Send + Sync {
    fn id(&self) -> &str;

    async fn tool_execute_before(
        &self,
        _input: &ToolExecuteBeforeInput,
        _output: &mut ToolExecuteBeforeOutput,
    ) -> Result<()> {
        Ok(())
    }

    async fn tool_execute_after(
        &self,
        _input: &ToolExecuteAfterInput,
        _output: &mut ToolExecuteAfterOutput,
    ) -> Result<()> {
        Ok(())
    }

    async fn chat_system_transform(
        &self,
        _input: &ChatSystemTransformInput,
        _output: &mut ChatSystemTransformOutput,
    ) -> Result<()> {
        Ok(())
    }

    async fn session_compacting(
        &self,
        _input: &SessionCompactingInput,
        _output: &mut SessionCompactingOutput,
    ) -> Result<()> {
        Ok(())
    }
}

pub struct HookRegistry {
    hooks: Vec<Arc<dyn SessionHook>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: Arc<dyn SessionHook>) {
        self.hooks.push(hook);
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub async fn run_tool_execute_before(
        &self,
        input: &ToolExecuteBeforeInput,
        output: &mut ToolExecuteBeforeOutput,
    ) -> Result<()> {
        for hook in &self.hooks {
            if let Err(e) = hook.tool_execute_before(input, output).await {
                tracing::error!(
                    target: code_plus_plus_telemetry::TOOL,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    hook = %hook.id(),
                    tool = %input.tool,
                    session_id = %input.session_id,
                    call_id = %input.call_id,
                    error = %e,
                    "hook tool_execute_before failed"
                );
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn run_tool_execute_after(
        &self,
        input: &ToolExecuteAfterInput,
        output: &mut ToolExecuteAfterOutput,
    ) -> Result<()> {
        for hook in &self.hooks {
            if let Err(e) = hook.tool_execute_after(input, output).await {
                tracing::error!(
                    target: code_plus_plus_telemetry::TOOL,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    hook = %hook.id(),
                    tool = %input.tool,
                    session_id = %input.session_id,
                    call_id = %input.call_id,
                    error = %e,
                    "hook tool_execute_after failed"
                );
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn run_chat_system_transform(
        &self,
        input: &ChatSystemTransformInput,
        output: &mut ChatSystemTransformOutput,
    ) -> Result<()> {
        for hook in &self.hooks {
            if let Err(e) = hook.chat_system_transform(input, output).await {
                tracing::error!(
                    target: code_plus_plus_telemetry::APP,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    hook = %hook.id(),
                    model = %input.model,
                    error = %e,
                    "hook chat_system_transform failed"
                );
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn run_session_compacting(
        &self,
        input: &SessionCompactingInput,
        output: &mut SessionCompactingOutput,
    ) -> Result<()> {
        for hook in &self.hooks {
            if let Err(e) = hook.session_compacting(input, output).await {
                tracing::error!(
                    target: code_plus_plus_telemetry::APP,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    hook = %hook.id(),
                    session_id = %input.session_id,
                    error = %e,
                    "hook session_compacting failed"
                );
                return Err(e);
            }
        }
        Ok(())
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountHook {
        id: String,
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SessionHook for CountHook {
        fn id(&self) -> &str {
            &self.id
        }

        async fn tool_execute_before(
            &self,
            _input: &ToolExecuteBeforeInput,
            output: &mut ToolExecuteBeforeOutput,
        ) -> Result<()> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            output.args = serde_json::json!({ "seen": self.id() });
            Ok(())
        }
    }

    #[test]
    fn hook_registry_runs_in_order() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(CountHook {
            id: "a".into(),
            counter: counter.clone(),
        }));
        registry.register(Arc::new(CountHook {
            id: "b".into(),
            counter: counter.clone(),
        }));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut out = ToolExecuteBeforeOutput {
                args: serde_json::json!({}),
            };
            registry
                .run_tool_execute_before(
                    &ToolExecuteBeforeInput {
                        tool: "read".into(),
                        session_id: "sess".into(),
                        call_id: "call".into(),
                    },
                    &mut out,
                )
                .await
                .unwrap();
            assert_eq!(counter.load(Ordering::SeqCst), 2);
            assert_eq!(out.args["seen"], "b");
        });
    }
}
