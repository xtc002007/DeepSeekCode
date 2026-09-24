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
        crate::axis::framing::sanitize_llm_error(raw)
    }
}
