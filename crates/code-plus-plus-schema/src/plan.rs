use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    Draft,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PlanMeta {
    pub plan_id: String,
    pub session_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub primary_path: String,
    pub status: PlanStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PlanCompletedPayload {
    pub plan_id: String,
    pub plan_path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct SessionPlanSummary {
    pub plan_id: String,
    pub plan_path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub completed_at: String,
    pub implement_prompt_dismissed: bool,
}
