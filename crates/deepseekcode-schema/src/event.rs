use chrono::{DateTime, Utc};
use deepseekcode_types::{MessageId, PermissionId, SessionId, ToolCallId};
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionCreated,
    PromptAdmitted,
    Prompted,
    MessageDelta,
    MessageCompleted,
    ToolStarted,
    ToolCompleted,
    ProviderTurnStarted,
    PermissionRequested,
    PolicyPrompted,
    ContextUpdated,
    SessionCompacted,
    SessionInterrupted,
    SessionIdle,
    McpStatusChanged,
    RevertCommitted,
    TodoUpdated,
    ToolProgress,
    PermissionResolved,
    PolicyResolved,
    QuestionRequested,
    PlanCompleted,
    ActivityUpdated,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreated => "session.created",
            Self::PromptAdmitted => "prompt.admitted",
            Self::Prompted => "prompted",
            Self::MessageDelta => "message.delta",
            Self::MessageCompleted => "message.completed",
            Self::ToolStarted => "tool.started",
            Self::ToolCompleted => "tool.completed",
            Self::ProviderTurnStarted => "provider.turn.started",
            Self::PermissionRequested => "permission.requested",
            Self::PolicyPrompted => "policy.prompted",
            Self::ContextUpdated => "context.updated",
            Self::SessionCompacted => "session.compacted",
            Self::SessionInterrupted => "session.interrupted",
            Self::SessionIdle => "session.idle",
            Self::McpStatusChanged => "mcp.status.changed",
            Self::RevertCommitted => "revert.committed",
            Self::TodoUpdated => "todo.updated",
            Self::ToolProgress => "tool.progress",
            Self::PermissionResolved => "permission.resolved",
            Self::PolicyResolved => "policy.resolved",
            Self::QuestionRequested => "question.requested",
            Self::PlanCompleted => "plan.completed",
            Self::ActivityUpdated => "activity.updated",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "session.created" | "sessioncreated" => Some(Self::SessionCreated),
            "prompt.admitted" | "promptadmitted" => Some(Self::PromptAdmitted),
            "prompted" => Some(Self::Prompted),
            "message.delta" | "messagedelta" => Some(Self::MessageDelta),
            "message.completed" | "messagecompleted" => Some(Self::MessageCompleted),
            "tool.started" | "toolstarted" => Some(Self::ToolStarted),
            "tool.completed" | "toolcompleted" => Some(Self::ToolCompleted),
            "provider.turn.started" | "providerturnstarted" => Some(Self::ProviderTurnStarted),
            "permission.requested" | "permissionrequested" => Some(Self::PermissionRequested),
            "policy.prompted" | "policyprompted" => Some(Self::PolicyPrompted),
            "context.updated" | "contextupdated" => Some(Self::ContextUpdated),
            "session.compacted" | "sessioncompacted" => Some(Self::SessionCompacted),
            "session.interrupted" | "sessioninterrupted" => Some(Self::SessionInterrupted),
            "session.idle" | "sessionidle" => Some(Self::SessionIdle),
            "mcp.status.changed" | "mcpstatuschanged" => Some(Self::McpStatusChanged),
            "revert.committed" | "revertcommitted" => Some(Self::RevertCommitted),
            "todo.updated" | "todoupdated" => Some(Self::TodoUpdated),
            "tool.progress" | "toolprogress" => Some(Self::ToolProgress),
            "permission.resolved" | "permissionresolved" => Some(Self::PermissionResolved),
            "policy.resolved" | "policyresolved" => Some(Self::PolicyResolved),
            "question.requested" | "questionrequested" => Some(Self::QuestionRequested),
            "plan.completed" | "plancompleted" => Some(Self::PlanCompleted),
            "activity.updated" | "activityupdated" => Some(Self::ActivityUpdated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct EventEnvelope {
    pub seq: i64,
    pub kind: EventKind,
    #[schema(value_type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub session_id: Option<SessionId>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptAdmittedPayload {
    pub message_id: MessageId,
    pub delivery: crate::session::PromptDelivery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaPayload {
    pub message_id: MessageId,
    pub delta: String,
    pub part_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStartedPayload {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCompletedPayload {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub output: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequestedPayload {
    pub permission_id: PermissionId,
    pub tool_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoUpdatedPayload {
    pub todos: Vec<crate::todo::TodoItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgressPayload {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub chunk: String,
    #[serde(default = "default_stdout_stream")]
    pub stream: String,
}

fn default_stdout_stream() -> String {
    "stdout".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResolvedPayload {
    pub permission_id: PermissionId,
    pub tool_name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequestedPayload {
    pub tool_call_id: ToolCallId,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "default_allow_custom")]
    pub allow_custom: bool,
}

fn default_allow_custom() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityUpdatedPayload {
    pub since_seq: i64,
}
