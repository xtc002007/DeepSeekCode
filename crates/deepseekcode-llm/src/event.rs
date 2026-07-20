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
    Completed,
    Error(String),
}
