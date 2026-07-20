use chrono::{DateTime, Utc};
use deepseekcode_types::PermissionId;
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
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub id: PermissionId,
    pub tool_name: String,
    pub description: String,
    pub input: serde_json::Value,
    pub status: PermissionStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PermissionResponse {
    pub status: PermissionStatus,
}
