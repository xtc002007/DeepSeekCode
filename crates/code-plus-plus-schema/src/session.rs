use crate::agent::AgentKind;
use chrono::{DateTime, Utc};
use code_plus_plus_types::{LocationId, MessageId, SessionId};
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
    pub id: SessionId,

    pub location_id: LocationId,
    pub title: Option<String>,
    pub agent: AgentKind,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,

    pub parent_session_id: Option<SessionId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct SessionInput {
    pub id: MessageId,

    pub session_id: SessionId,
    pub prompt: String,
    pub delivery: PromptDelivery,
    pub admitted_at: DateTime<Utc>,
    pub promoted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct CreateSessionRequest {
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
    pub id: Option<MessageId>,

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
    pub session_id: SessionId,
    pub activity: SessionActivityType,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertStageRequest {
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertFileEntry {
    pub path: String,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertState {
    pub staged_message_id: MessageId,
    pub staged_at: DateTime<Utc>,
    pub files: Vec<RevertFileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
