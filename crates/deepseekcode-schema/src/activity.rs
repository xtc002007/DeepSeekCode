use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    UserMessage,
    AssistantMessage,
    Reasoning,
    Read,
    Write,
    Edit,
    ApplyPatch,
    Grep,
    Glob,
    List,
    Bash,
    WebFetch,
    WebSearch,
    Skill,
    Lsp,
    Mcp,
    TodoWrite,
    TodoRead,
    Task,
    Question,
    PlanReady,
    Permission,
    Context,
    SessionMeta,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityGroupKind {
    ProviderTurn,
    TaskSubagent,
    Thought,
    SessionMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// Running past the stale timeout without close events (unclosed turn).
    /// Terminal for busy indicators; purely presentational, never persisted
    /// as a new event kind.
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityTargetKind {
    File {
        path: String,
        line_start: Option<u32>,
        line_end: Option<u32>,
    },
    Command {
        command: String,
    },
    Pattern {
        pattern: String,
        scope: Option<String>,
    },
    Url {
        url: String,
    },
    Session {
        session_id: String,
        label: Option<String>,
    },
    TodoList,
    Skill {
        name: String,
    },
    Generic {
        label: String,
    },
    None,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct DiffStats {
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ActivityMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_stats: Option<DiffStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_pending: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_in_progress: Option<u32>,
    /// Primary todo row to highlight when this todo activity leaf is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_focus_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_match_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_match_line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ActivityLeaf {
    pub id: String,
    pub kind: ActivityKind,
    pub verb: String,
    pub target: ActivityTargetKind,
    #[serde(default)]
    pub meta: ActivityMeta,
    pub status: ActivityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Wall-clock when this leaf entered Running (event created_at).
    /// Used to render Stale after timeout without new events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ActivitySummary {
    pub files_edited: u32,
    pub files_explored: u32,
    pub searches: u32,
    pub commands: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ActivityGroup {
    pub id: String,
    pub kind: ActivityGroupKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ActivitySummary>,
    #[schema(no_recursion)]
    pub children: Vec<ActivityNode>,
    #[serde(default)]
    pub collapsed_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum ActivityNode {
    Group(ActivityGroup),
    Leaf(ActivityLeaf),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, JsonSchema)]
pub struct ActivityTree {
    #[schema(no_recursion)]
    pub roots: Vec<ActivityNode>,
}
