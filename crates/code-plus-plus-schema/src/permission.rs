use chrono::{DateTime, Utc};
use code_plus_plus_types::PermissionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionStatus {
    Pending,
    Allowed,
    Denied,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PermissionRequest {
    pub id: PermissionId,
    pub tool_name: String,
    pub description: String,
    pub input: serde_json::Value,
    pub status: PermissionStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PermissionResponse {
    pub status: PermissionStatus,
}
