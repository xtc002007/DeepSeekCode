use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_priority() -> String {
    "medium".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct TodoUpdateRequest {
    pub todos: Vec<TodoItem>,
}
