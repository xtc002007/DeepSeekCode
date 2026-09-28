use chrono::{DateTime, Utc};
use deepseekcode_types::{PermissionId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Policy decision for a scope/pattern pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyAction {
    #[serde(alias = "allow")]
    Permit,
    #[serde(alias = "ask")]
    Prompt,
    #[serde(alias = "deny")]
    Block,
}

impl PolicyAction {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "permit" | "allow" => Some(Self::Permit),
            "prompt" | "ask" => Some(Self::Prompt),
            "block" | "deny" => Some(Self::Block),
            _ => None,
        }
    }
}

/// A single rule in an ordered ruleset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PolicyRule {
    pub scope: String,
    pub pattern: String,
    pub action: PolicyAction,
}

pub type PolicyRuleset = Vec<PolicyRule>;

/// Outcome of evaluating all patterns for one tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Permit,
    Prompt,
    Block,
}

/// User reply to a policy prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyReplyKind {
    Once,
    Project,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PolicyReply {
    pub reply: PolicyReplyKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Pending or resolved policy prompt shown in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PolicyRequest {
    pub id: PermissionId,

    pub session_id: SessionId,
    pub scope: String,
    pub tool_name: String,
    pub patterns: Vec<String>,
    /// Patterns that would be granted when the user chooses "project".
    pub remember_patterns: Vec<String>,
    pub description: String,
    pub input: serde_json::Value,
    pub status: PolicyRequestStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyRequestStatus {
    Pending,
    Permitted,
    Rejected,
}

/// Project-persisted user grant.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct PolicyGrant {
    pub id: String,
    pub scope: String,
    pub pattern: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPromptedPayload {
    pub policy_id: PermissionId,
    pub session_id: SessionId,
    pub scope: String,
    pub tool_name: String,
    pub patterns: Vec<String>,
    pub remember_patterns: Vec<String>,
    pub description: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResolvedPayload {
    pub policy_id: PermissionId,
    pub session_id: SessionId,
    pub scope: String,
    pub tool_name: String,
    pub reply: PolicyReplyKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
