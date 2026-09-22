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
