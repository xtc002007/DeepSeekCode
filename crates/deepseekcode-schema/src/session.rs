use crate::agent::AgentKind;
use chrono::{DateTime, Utc};
use deepseekcode_types::{LocationId, MessageId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum PromptDelivery {
    #[default]
    Queue,
    Steer,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct Session {
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub id: SessionId,
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub location_id: LocationId,
    pub title: Option<String>,
    pub agent: AgentKind,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    #[schema(value_type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub parent_session_id: Option<SessionId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct SessionInput {
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub id: MessageId,
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub session_id: SessionId,
    pub prompt: String,
    pub delivery: PromptDelivery,
    pub admitted_at: DateTime<Utc>,
    pub promoted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct CreateSessionRequest {
    #[schema(value_type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub id: Option<SessionId>,
    pub directory: String,
    pub agent: Option<AgentKind>,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct UpdateSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PromptRequest {
    #[schema(value_type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub id: Option<MessageId>,
    #[schema(value_type = String)]
    #[schemars(with = "String")]
    pub session_id: SessionId,
    pub prompt: String,
    #[serde(default)]
    pub delivery: PromptDelivery,
    #[serde(default = "default_resume")]
    pub resume: bool,
}

fn default_resume() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionActivityType {
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionActivity {
    #[schema(value_type = String)]
    pub session_id: SessionId,
    pub activity: SessionActivityType,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertStageRequest {
    #[schema(value_type = String)]
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertFileEntry {
    pub path: String,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertState {
    #[schema(value_type = String)]
    pub staged_message_id: MessageId,
    pub staged_at: DateTime<Utc>,
    pub files: Vec<RevertFileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
