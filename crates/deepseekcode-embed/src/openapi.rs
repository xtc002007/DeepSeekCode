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

// P1-10: single source of truth in `deepseekcode-schema::api`; re-exported
// here so existing `crate::openapi::X` paths keep compiling.
pub use deepseekcode_schema::api::{
    AddProjectDirectoryRequest, CreatePtyRequest, CreateWorktreeRequest, ForkRequest,
    McpConnectResponse, MoveSessionRequest, OkResponse, PluginInfo, ProjectDirectoryEntry,
    SessionExportResponse, SkillInfo, TimelineEntry, UpdateModelRequest, WorktreeEntry,
};

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
        crate::routes::session_cache_status,
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
        crate::routes::legacy_list_policy_grants,
        crate::routes::legacy_delete_policy_grant,
        crate::routes::legacy_update_policy_config,
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
        crate::routes::destroy_pty,
        crate::routes::pty_ticket,
        crate::routes::pty_connect,
        crate::routes::agent_pty_connect,
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
    let spec = ApiDoc::openapi();
    tracing::info!(
        target: deepseekcode_telemetry::SERVER,
        run_id = %deepseekcode_telemetry::current_run_id(),
        paths = spec.paths.paths.len(),
        "openapi spec generated"
    );
    spec
}
