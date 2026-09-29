use async_trait::async_trait;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::time::Duration;

use super::registry::Tool;
use super::services::{current_services, wait_for_question_answer};

pub struct QuestionTool;

#[async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }
    fn description(&self) -> &str {
        "Ask the user a question and wait for an answer"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "Question text shown to the user" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Predefined choices; Desktop appends an 'Other (custom input)' option with a text field when non-empty"
                },
                "allow_custom": {
                    "type": "boolean",
                    "default": true,
                    "description": "When true (default), Desktop always offers a final custom-input path alongside options"
                }
            },
            "required": ["question"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "question",
            question_len = input["question"].as_str().map(|s| s.len()).unwrap_or(0),
            "question started"
        );
        let question = input["question"].as_str().unwrap_or("").to_string();
        let allow_custom = input
            .get("allow_custom")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let services = current_services().ok_or_else(|| ToolFailure {
            message: "question channel not configured".into(),
            retryable: false,
        })?;
        if let Some(events) = &services.events {
            let _ = events
                .append(
                    EventKind::QuestionRequested,
                    Some(ctx.session_id),
                    json!({
                        "tool_call_id": ctx.tool_call_id.to_string(),
                        "question": question,
                        "options": input.get("options").cloned().unwrap_or(json!([])),
                        "allow_custom": allow_custom,
                    }),
                )
                .await;
        }
        let answer = wait_for_question_answer(&services, Duration::from_secs(300))
            .await
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?;
        Ok(json!({ "answer": answer }))
    }
}
