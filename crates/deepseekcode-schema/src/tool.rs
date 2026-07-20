use crate::todo::TodoItem;
use async_trait::async_trait;
use deepseekcode_types::{MessageId, SessionId, ToolCallId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

/// Todo persistence injected into `ToolContext` by `ToolRegistry`.
#[async_trait]
pub trait TodoStoreOps: Send + Sync {
    async fn update_todos(
        &self,
        session_id: SessionId,
        todos: &[TodoItem],
    ) -> std::result::Result<Vec<TodoItem>, String>;

    async fn get_todos(&self, session_id: SessionId) -> std::result::Result<Vec<TodoItem>, String>;
}

#[derive(Clone, Default)]
pub struct ToolContextExtras {
    pub todo_store: Option<Arc<dyn TodoStoreOps>>,
}

impl std::fmt::Debug for ToolContextExtras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContextExtras")
            .field("todo_store_configured", &self.todo_store.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub session_id: SessionId,
    pub agent: String,
    pub assistant_message_id: MessageId,
    pub tool_call_id: ToolCallId,
    pub working_directory: String,
    #[serde(skip, default)]
    pub extras: ToolContextExtras,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolFailure {
    pub message: String,
    pub retryable: bool,
}

/// Structured tool result for events, messages, and activity projection.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolResultEnvelope {
    pub title: String,
    #[schema(value_type = serde_json::Value)]
    pub output: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = serde_json::Value)]
    pub metadata: serde_json::Value,
    pub duration_ms: u64,
}

impl ToolResultEnvelope {
    pub fn new(title: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            title: title.into(),
            output,
            metadata: serde_json::Value::Object(Default::default()),
            duration_ms: 0,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    /// Parse stored tool output — accepts legacy bare JSON or envelope form.
    pub fn from_stored(value: &serde_json::Value) -> Self {
        if let (Some(title), Some(output)) = (
            value.get("title").and_then(|v| v.as_str()),
            value.get("output"),
        ) {
            return Self {
                title: title.to_string(),
                output: output.clone(),
                metadata: value
                    .get("metadata")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default())),
                duration_ms: value
                    .get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            };
        }
        Self::new("tool result", value.clone())
    }

    pub fn to_stored(&self) -> serde_json::Value {
        serde_json::json!({
            "title": self.title,
            "output": self.output,
            "metadata": self.metadata,
            "duration_ms": self.duration_ms,
        })
    }
}
