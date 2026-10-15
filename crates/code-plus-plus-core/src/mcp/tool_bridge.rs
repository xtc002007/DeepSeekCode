use async_trait::async_trait;
use code_plus_plus_schema::tool::{ToolContext, ToolFailure};
use serde_json::Value;
use std::sync::Arc;

use super::local_stdio::McpToolInfo;
use super::pool::McpConnectionPool;
use crate::tools::registry::Tool;
use crate::tools::services::{ensure_not_cancelled, race_cancel};

pub fn bridged_tool_name(server: &str, tool: &str) -> String {
    format!("mcp_{server}_{tool}")
}

pub fn bridged_tools_from_pool(pool: &Arc<McpConnectionPool>) -> Vec<Box<dyn Tool>> {
    let connected = pool.connected_tools();
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    for (server, tool_infos) in connected {
        for tool in tool_infos {
            tools.push(Box::new(McpBridgedTool::new(
                pool.clone(),
                server.clone(),
                tool,
            )));
        }
    }
    tools
}

pub struct McpBridgedTool {
    pool: Arc<McpConnectionPool>,
    server: String,
    mcp_tool_name: String,
    registry_name: String,
    description: String,
    input_schema: Value,
}

impl McpBridgedTool {
    pub fn new(pool: Arc<McpConnectionPool>, server: String, tool: McpToolInfo) -> Self {
        let registry_name = bridged_tool_name(&server, &tool.name);
        Self {
            pool,
            server,
            mcp_tool_name: tool.name,
            registry_name,
            description: tool.description,
            input_schema: tool.input_schema,
        }
    }
}

#[async_trait]
impl Tool for McpBridgedTool {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            tool = self.name(),
            server = %self.server,
            "mcp bridged tool started"
        );
        let r = async {
            ensure_not_cancelled()?;
            let pool = self.pool.clone();
            let server = self.server.clone();
            let tool = self.mcp_tool_name.clone();
            race_cancel(async move {
                pool.call_tool(&server, &tool, input)
                    .await
                    .map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: true,
                    })
            })
            .await
        }
        .await;
        match &r {
            Ok(v) => tracing::debug!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                tool = self.name(),
                output_bytes = serde_json::to_string(v).map(|s| s.len()).unwrap_or(0),
                "mcp bridged tool succeeded"
            ),
            Err(e) => tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                tool = self.name(),
                server = %self.server,
                error = %e.message,
                "mcp bridged tool failed"
            ),
        }
        r
    }
}
