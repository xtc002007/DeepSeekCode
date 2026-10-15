#[derive(Debug, Clone)]
pub enum LlmEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallDelta {
        index: usize,
        id: String,
        name: String,
        arguments: String,
    },
    /// Provider-reported token accounting (DeepSeek cache hit/miss included).
    /// Emitted when an SSE frame carries a `usage` object; consumers should
    /// prefer it over local estimates.
    Usage(crate::request::TokenUsage),
    Completed,
    Error(String),
}

impl LlmEvent {
    /// Sanitized display for SSE/logs: caps length and redacts key-like
    /// fragments so provider error text never leaks credentials (P0-5).
    pub fn sanitized_error(raw: &str) -> String {
        let sanitized = crate::axis::framing::sanitize_llm_error(raw);
        if raw.len() > sanitized.len() || raw.len() > 500 {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                raw_len = raw.len(),
                sanitized_len = sanitized.len(),
                error_summary = %code_plus_plus_telemetry::summarize_prompt(&sanitized, false),
                "llm event error truncated/redacted for log"
            );
        } else {
            tracing::debug!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                sanitized_len = sanitized.len(),
                "llm event error sanitized"
            );
        }
        if crate::overflow::is_context_overflow(&sanitized) {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                error_summary = %code_plus_plus_telemetry::summarize_prompt(&sanitized, false),
                "llm event error indicates context overflow"
            );
        }
        tracing::info!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            kind = "error_sanitized",
            "llm event sanitized"
        );
        if sanitized.trim().is_empty() && !raw.trim().is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                raw_len = raw.len(),
                "llm event sanitization produced empty error"
            );
        }
        sanitized
    }

    /// Product-grade event-kind log (no payload content, only shape).
    pub fn log_kind(&self) {
        let kind = match self {
            LlmEvent::TextDelta(_) => "text_delta",
            LlmEvent::ReasoningDelta(_) => "reasoning_delta",
            LlmEvent::ToolCallDelta {
                index, id, name, ..
            } => {
                tracing::debug!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    index,
                    tool_id = %id,
                    tool_name = %name,
                    "llm event tool_call_delta"
                );
                "tool_call_delta"
            }
            LlmEvent::Usage(u) => {
                tracing::info!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    input_tokens = ?u.input_tokens,
                    output_tokens = ?u.output_tokens,
                    "llm event usage"
                );
                "usage"
            }
            LlmEvent::Completed => "completed",
            LlmEvent::Error(_) => {
                tracing::error!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    "llm event error emitted"
                );
                "error"
            }
        };
        tracing::debug!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            kind,
            "llm event kind"
        );
    }
}
