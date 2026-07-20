use chrono::{DateTime, Utc};
use deepseekcode_types::{MessageId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
    Compaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PartKind {
    Text,
    Reasoning,
    ToolCall,
    ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct MessagePart {
    pub kind: PartKind,
    pub text: Option<String>,
    #[schema(value_type = Option<String>)]
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_output: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct Message {
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub id: MessageId,
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub session_id: SessionId,
    pub role: MessageRole,
    pub parts: Vec<MessagePart>,
    #[serde(default)]
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}
