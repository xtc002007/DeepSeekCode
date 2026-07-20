use thiserror::Error;

pub type Result<T> = std::result::Result<T, DeepSeekCodeError>;

#[derive(Debug, Error)]
pub enum DeepSeekCodeError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("context unavailable: {0}")]
    ContextUnavailable(String),

    #[error("session interrupted")]
    SessionInterrupted,

    #[error("tool failure: {0}")]
    ToolFailure(String),

    #[error("llm error: {0}")]
    Llm(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("retry provider turn after compaction")]
    RetryAfterCompaction,

    #[error("{0}")]
    Other(String),
}
