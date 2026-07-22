use utoipa::OpenApi;

use deepseekcode_schema::activity::{ActivityGroup, ActivityLeaf, ActivityNode, ActivityTree};
use deepseekcode_schema::event::EventEnvelope;
use deepseekcode_schema::message::Message;
use deepseekcode_schema::permission::{PermissionRequest, PermissionResponse};
use deepseekcode_schema::policy::{PolicyGrant, PolicyReply, PolicyRequest};
use deepseekcode_schema::project::{
    CreateProjectRequest, FsDiffResponse, FsFileResponse, FsTreeResponse, HealthResponse, Project,
    PtyCreateResponse, PtyTicketResponse,
};
use deepseekcode_schema::provider::{ConnectProviderRequest, ProviderInfo};
use deepseekcode_schema::session::{
    CreateSessionRequest, PromptRequest, RevertStageRequest, RevertState, Session, SessionActivity,
    UpdateSessionRequest,
};
use deepseekcode_schema::todo::{TodoItem, TodoUpdateRequest};
use deepseekcode_types::MessageId;

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct MoveSessionRequest {
    pub directory: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateModelRequest {
    pub agent: Option<String>,
    pub model_id: String,
    pub provider_id: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct SessionExportResponse {
    pub version: i32,
    pub session: Session,
    pub messages: Vec<Message>,
    pub timeline: Vec<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TimelineEntry {
    pub seq: i64,
    pub kind: String,
    pub label: String,
    pub detail: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct WorktreeEntry {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ProjectDirectoryEntry {
    pub id: String,
    pub project_path: String,
    pub directory_path: String,
    pub strategy: String,
    pub created_at: String,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct AddProjectDirectoryRequest {
    pub directory_path: String,
    pub strategy: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateWorktreeRequest {
    pub branch: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct McpConnectResponse {
    pub ok: bool,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ForkRequest {
    #[schema(value_type = Option<String>)]
    pub until_message_id: Option<MessageId>,
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct CreatePtyRequest {
    pub rows: Option<u16>,
    pub cols: Option<u16>,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "DeepSeekCode API",
        version = env!("CARGO_PKG_VERSION"),
        description = "HTTP API for DeepSeekCode sessions, filesystem, events, and PTY."
    ),
    paths(
        crate::routes::health_root,
        crate::routes::health,
        crate::routes::openapi_json,
        crate::routes::get_location,
        crate::routes::list_sessions,
        crate::routes::create_session,
        crate::routes::get_session,
        crate::routes::update_session,
        crate::routes::delete_session,
        crate::routes::list_active_sessions,
        crate::routes::prompt,
        crate::routes::interrupt,
        crate::routes::list_messages,
        crate::routes::list_timeline,
        crate::routes::list_activity,
        crate::routes::get_session_todo,
        crate::routes::update_session_todo,
        crate::routes::list_session_plans,
        crate::routes::dismiss_session_plan_prompt,
        crate::routes::answer_session_question,
        crate::routes::list_session_events,
        crate::routes::session_event_head,
        crate::routes::compact_session,
        crate::routes::fork_session,
        crate::routes::move_session,
        crate::routes::export_session,
        crate::routes::revert_get_staged,
        crate::routes::revert_stage,
        crate::routes::revert_clear,
        crate::routes::revert_commit,
        crate::routes::update_session_model,
        crate::routes::list_providers,
        crate::routes::connect_provider,
        crate::routes::disconnect_provider,
        crate::routes::list_pending_permissions,
        crate::routes::respond_permission,
        crate::routes::list_pending_policies,
        crate::routes::reply_policy,
        crate::routes::list_policy_grants,
        crate::routes::delete_policy_grant,
        crate::routes::update_policy_config,
        crate::routes::list_mcp,
        crate::routes::connect_mcp,
        crate::routes::disconnect_mcp,
        crate::routes::list_skills,
        crate::routes::list_plugins,
        crate::routes::list_project_directories,
        crate::routes::add_project_directory,
        crate::routes::list_worktrees,
        crate::routes::create_worktree,
        crate::routes::remove_worktree,
        crate::routes::prune_worktrees,
        crate::routes::export_diagnostics,
        crate::routes::event_stream,
        crate::routes::fs_tree,
        crate::routes::fs_file,
        crate::routes::fs_write,
        crate::routes::fs_diff,
        crate::routes::create_pty,
        crate::routes::resize_pty,
        crate::routes::pty_ticket,
        crate::routes::pty_connect,
        crate::routes::list_projects,
        crate::routes::create_project,
    ),
    components(schemas(
        HealthResponse,
        Session,
        CreateSessionRequest,
        UpdateSessionRequest,
        PromptRequest,
        Message,
        ProviderInfo,
        ConnectProviderRequest,
        PermissionRequest,
        PermissionResponse,
        PolicyRequest,
        PolicyReply,
        PolicyGrant,
        EventEnvelope,
        Project,
        CreateProjectRequest,
        FsTreeResponse,
        FsFileResponse,
        FsDiffResponse,
        PtyCreateResponse,
        PtyTicketResponse,
        SessionActivity,
        ActivityTree,
        ActivityLeaf,
        ActivityGroup,
        ActivityNode,
        TodoItem,
        TodoUpdateRequest,
        deepseekcode_schema::tool::ToolResultEnvelope,
        OkResponse,
        MoveSessionRequest,
        UpdateModelRequest,
        SkillInfo,
        SessionExportResponse,
        TimelineEntry,
        RevertStageRequest,
        RevertState,
        WorktreeEntry,
        ProjectDirectoryEntry,
        AddProjectDirectoryRequest,
        CreateWorktreeRequest,
        McpConnectResponse,
        ForkRequest,
        CreatePtyRequest,
        deepseekcode_schema::plan::SessionPlanSummary,
        deepseekcode_schema::plan::PlanMeta,
        deepseekcode_schema::plan::PlanCompletedPayload,
    )),
    tags(
        (name = "health", description = "Health checks"),
        (name = "meta", description = "API metadata"),
        (name = "location", description = "Workspace location"),
        (name = "session", description = "Session lifecycle"),
        (name = "provider", description = "LLM providers"),
        (name = "permission", description = "Tool permissions"),
        (name = "policy", description = "Policy prompts and grants"),
        (name = "mcp", description = "Model Context Protocol servers"),
        (name = "skills", description = "Agent skills"),
        (name = "plugins", description = "Plugin host"),
        (name = "worktree", description = "Git worktrees and project directories"),
        (name = "diagnostics", description = "Diagnostic export"),
        (name = "event", description = "Server-sent events"),
        (name = "fs", description = "Filesystem access"),
        (name = "pty", description = "Pseudo-terminal"),
        (name = "projects", description = "Recent projects"),
    )
)]
pub struct ApiDoc;

pub fn spec() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
