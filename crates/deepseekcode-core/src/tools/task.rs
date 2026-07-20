use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use deepseekcode_types::{SessionId, ToolCallId};
use serde_json::{json, Value};

use super::registry::Tool;
use super::services::current_services;

pub struct TaskTool;

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }
    fn description(&self) -> &str {
        "Spawn a subagent task in a child session"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": { "type": "string" },
                "prompt": { "type": "string" },
                "title": { "type": "string" }
            },
            "required": ["agent", "prompt"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let agent = input["agent"].as_str().unwrap_or("build").to_string();
        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
        let title = input["title"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| prompt.chars().take(48).collect());
        let services = current_services().ok_or_else(|| ToolFailure {
            message: "task spawner not configured".into(),
            retryable: false,
        })?;
        let spawner = services.task_spawner.as_ref().ok_or_else(|| ToolFailure {
            message: "task spawner not configured".into(),
            retryable: false,
        })?;
        emit_task_progress(
            &services,
            ctx.session_id,
            &ctx.tool_call_id,
            "spawning",
        )
        .await;
        let child_id = spawner
            .spawn_task(ctx.session_id, &agent, &prompt)
            .await
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?;
        emit_task_progress(
            &services,
            ctx.session_id,
            &ctx.tool_call_id,
            &format!("child session {} running", child_id),
        )
        .await;
        Ok(json!({
            "session_id": child_id.to_string(),
            "agent": agent,
            "title": title,
            "prompt": prompt,
            "parent_session_id": ctx.session_id.to_string(),
            "status": "spawned",
        }))
    }
}

async fn emit_task_progress(
    services: &super::services::ToolServices,
    session_id: SessionId,
    tool_call_id: &ToolCallId,
    message: &str,
) {
    if let Some(progress) = &services.progress {
        let _ = progress
            .progress(session_id, tool_call_id, "task", message, "status")
            .await;
    }
}
