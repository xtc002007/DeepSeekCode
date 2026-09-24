//! Canonical HTTP API shapes (P1-10 single source of truth).
//!
//! These types previously lived in `deepseekcode-server/src/openapi.rs`,
//! forcing the SDK to hand-roll `json!` bodies that drifted silently on
//! renames. They now live here; the server re-exports them for back-compat.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::message::Message;
use crate::session::Session;
use deepseekcode_types::MessageId;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct MoveSessionRequest {
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct UpdateModelRequest {
    pub agent: Option<String>,
    pub model_id: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct SessionExportResponse {
    pub version: i32,
    pub session: Session,
    pub messages: Vec<Message>,
    pub timeline: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct TimelineEntry {
    pub seq: i64,
    pub kind: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct WorktreeEntry {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ProjectDirectoryEntry {
    pub id: String,
    pub project_path: String,
    pub directory_path: String,
    pub strategy: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct AddProjectDirectoryRequest {
    pub directory_path: String,
    pub strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct CreateWorktreeRequest {
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct McpConnectResponse {
    pub ok: bool,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ForkRequest {
    #[schema(value_type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub until_message_id: Option<MessageId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct CreatePtyRequest {
    pub rows: Option<u16>,
    pub cols: Option<u16>,
}
