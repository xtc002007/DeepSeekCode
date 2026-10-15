use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl LlmToolCall {
    /// Strict arguments parsing: returns the parsed value or a typed error
    /// instead of silently substituting `{}` (P0-5). Callers decide whether
    /// to reject or repair; silent `{}` previously masked provider bugs.
    pub fn parse_arguments_strict(raw: &str) -> Result<Value, String> {
        tracing::debug!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            raw_summary = %code_plus_plus_telemetry::summarize_prompt(raw, false),
            "llm tool_call parse start"
        );
        if raw.trim().is_empty() {
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                "llm tool_call empty arguments; defaulting to empty object"
            );
            return Ok(Value::Object(Default::default()));
        }
        match serde_json::from_str(raw) {
            Ok(v) => {
                tracing::debug!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    raw_len = raw.len(),
                    "llm tool_call parse succeeded"
                );
                Ok(v)
            }
            Err(e) => {
                let err = format!("invalid tool arguments JSON: {e}");
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    error = %err,
                    raw_summary = %code_plus_plus_telemetry::summarize_prompt(raw, false),
                    "llm tool_call parse failed"
                );
                tracing::error!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    error = %err,
                    "llm tool_call arguments unparseable"
                );
                Err(err)
            }
        }
    }
}
