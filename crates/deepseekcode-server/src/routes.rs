use axum::http::{header, HeaderValue, StatusCode};
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
    routing::{delete, get, patch, post, put},
    Router,
};
use deepseekcode_schema::message::Message;
use deepseekcode_schema::permission::PermissionRequest;
use deepseekcode_schema::permission::PermissionResponse;
use deepseekcode_schema::policy::{PolicyGrant, PolicyReply, PolicyRequest};
use deepseekcode_schema::project::FsDiffResponse;
use deepseekcode_schema::project::FsFileResponse;
use deepseekcode_schema::project::{
    CreateProjectRequest, FsEntry, FsTreeResponse, HealthResponse, Project, PtyCreateResponse,
    PtyTicketResponse,
};
use deepseekcode_schema::provider::{ConnectProviderRequest, ProviderInfo};
use deepseekcode_schema::session::{
    CreateSessionRequest, PromptRequest, RevertStageRequest, RevertState, Session,
    UpdateSessionRequest,
};
use deepseekcode_storage::ProjectsStore;
use deepseekcode_types::{SessionId, DIAGNOSTICS_ARCHIVE_PREFIX};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::{Component, Path as StdPath, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use crate::openapi;
use crate::openapi::{
    AddProjectDirectoryRequest, CreatePtyRequest, CreateWorktreeRequest, ForkRequest,
    MoveSessionRequest, UpdateModelRequest,
};
use crate::pty::{PTY_TICKET_HEADER, PTY_TICKET_HEADER_VALUE};
use crate::state::AppState;

const SSE_REPLAY_LIMIT: i64 = 10_000;

fn sse_replay_limit() -> i64 {
    std::env::var("DEEPSEEKCODE_SSE_REPLAY_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(SSE_REPLAY_LIMIT)
}

fn parse_session_id(id: &str) -> Result<SessionId, (StatusCode, Json<serde_json::Value>)> {
    SessionId::from_str(id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid session id" })),
        )
    })
}

fn parse_permission_id(
    id: &str,
) -> Result<deepseekcode_types::PermissionId, (StatusCode, Json<serde_json::Value>)> {
    deepseekcode_types::PermissionId::from_str(id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid permission id" })),
        )
    })
}

fn service_error_response(
    e: deepseekcode_types::DeepSeekCodeError,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = match &e {
        deepseekcode_types::DeepSeekCodeError::NotFound(_) => StatusCode::NOT_FOUND,
        deepseekcode_types::DeepSeekCodeError::PermissionDenied(_) => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({ "error": e.to_string() })))
}

macro_rules! service_err {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => return service_error_response(error).into_response(),
        }
    };
}

macro_rules! parse_session_path {
    ($id:expr) => {
        match parse_session_id($id) {
            Ok(sid) => sid,
            Err(resp) => return resp.into_response(),
        }
    };
}

macro_rules! parse_permission_path {
    ($id:expr) => {
        match parse_permission_id($id) {
            Ok(pid) => pid,
            Err(resp) => return resp.into_response(),
        }
    };
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_root))
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi_json))
        .route("/api/v1/location", get(get_location))
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route(
            "/api/v1/project/directories",
            get(list_project_directories).post(add_project_directory),
        )
        .route(
            "/api/v1/worktree",
            get(list_worktrees)
                .post(create_worktree)
                .delete(remove_worktree),
        )
        .route("/api/v1/worktree/prune", post(prune_worktrees))
        .route("/api/v1/session", get(list_sessions).post(create_session))
        .route(
            "/api/v1/session/{id}",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route("/api/v1/session/active", get(list_active_sessions))
        .route("/api/v1/session/{id}/prompt", post(prompt))
        .route("/api/v1/session/{id}/interrupt", post(interrupt))
        .route("/api/v1/session/{id}/messages", get(list_messages))
        .route("/api/v1/session/{id}/timeline", get(list_timeline))
        .route("/api/v1/session/{id}/activity", get(list_activity))
        .route(
            "/api/v1/session/{id}/todo",
            get(get_session_todo).post(update_session_todo),
        )
        .route("/api/v1/session/{id}/plans", get(list_session_plans))
        .route(
            "/api/v1/session/{id}/plans/{plan_id}/dismiss",
            post(dismiss_session_plan_prompt),
        )
        .route(
            "/api/v1/session/{id}/question/answer",
            post(answer_session_question),
        )
        .route("/api/v1/session/{id}/events", get(list_session_events))
        .route("/api/v1/session/{id}/events/head", get(session_event_head))
        .route("/api/v1/session/{id}/compact", post(compact_session))
        .route("/api/v1/session/{id}/fork", post(fork_session))
        .route("/api/v1/session/{id}/revert", get(revert_get_staged))
        .route("/api/v1/session/{id}/revert/stage", post(revert_stage))
        .route("/api/v1/session/{id}/revert/clear", post(revert_clear))
        .route("/api/v1/session/{id}/revert/commit", post(revert_commit))
        .route("/api/v1/session/{id}/move", post(move_session))
        .route("/api/v1/session/{id}/export", get(export_session))
        .route("/api/v1/session/{id}/model", post(update_session_model))
        .route("/api/v1/provider", get(list_providers))
        .route("/api/v1/provider/{id}/connect", post(connect_provider))
        .route(
            "/api/v1/provider/{id}/disconnect",
            post(disconnect_provider),
        )
        .route("/api/v1/permission/pending", get(list_pending_permissions))
        .route("/api/v1/permission/{id}/respond", post(respond_permission))
        .route("/api/v1/policy/pending", get(list_pending_policies))
        .route("/api/v1/policy/{id}/reply", post(reply_policy))
        .route("/api/v1/policy/grants", get(list_policy_grants))
        .route("/api/v1/policy/grants/{id}", delete(delete_policy_grant))
        .route("/api/v1/policy/config", put(update_policy_config))
        .route("/api/v1/mcp", get(list_mcp))
        .route("/api/v1/mcp/{name}/connect", post(connect_mcp))
        .route("/api/v1/mcp/{name}/disconnect", post(disconnect_mcp))
        .route("/api/v1/skills", get(list_skills))
        .route("/api/v1/plugins", get(list_plugins))
        .route("/api/v1/diagnostics/export", get(export_diagnostics))
        .route("/api/v1/event", get(event_stream))
        .route("/api/v1/fs/tree", get(fs_tree))
        .route("/api/v1/fs/file", get(fs_file).put(fs_write))
        .route("/api/v1/fs/diff", get(fs_diff))
        .route("/api/v1/pty", post(create_pty))
        .route("/api/v1/pty/{id}/ticket", get(pty_ticket))
        .route("/api/v1/pty/{id}/resize", post(resize_pty))
        .route("/api/v1/pty/{id}/connect", get(pty_connect))
        .with_state(state)
}

fn health_response() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses((status = 200, description = "Service is healthy", body = HealthResponse))
)]
pub(crate) async fn health_root() -> impl IntoResponse {
    health_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "health",
    responses((status = 200, description = "Service is healthy", body = HealthResponse))
)]
pub(crate) async fn health() -> impl IntoResponse {
    health_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/openapi.json",
    tag = "meta",
    responses((status = 200, description = "OpenAPI specification document"))
)]
pub(crate) async fn openapi_json() -> impl IntoResponse {
    Json(openapi::spec())
}

#[utoipa::path(
    get,
    path = "/api/v1/location",
    tag = "location",
    responses((status = 200, description = "Current workspace location"))
)]
pub(crate) async fn get_location(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    Json(serde_json::json!({
        "directory": dir,
        "project_dir": engine.project_dir,
        "skills": engine.skills().into_iter().map(|s| serde_json::json!({
            "name": s.name,
            "description": s.description,
            "path": s.path.display().to_string(),
        })).collect::<Vec<_>>(),
    }))
}

fn project_dir(headers: &HeaderMap) -> String {
    let raw = headers
        .get("x-deepseekcode-directory")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string()
        });
    normalize_server_directory(&raw)
}

fn fs_write_allowed() -> bool {
    std::env::var("DEEPSEEKCODE_ALLOW_FS_WRITE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn normalize_server_directory(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
    }
    let pb = std::path::Path::new(path);
    let canonical = std::fs::canonicalize(pb).unwrap_or_else(|_| pb.to_path_buf());
    let s = canonical.display().to_string();
    #[cfg(windows)]
    {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

fn normalize_path(path: &StdPath) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

fn resolve_within_project(project_root: &str, requested: &str) -> Result<PathBuf, StatusCode> {
    let root = std::fs::canonicalize(project_root).map_err(|_| StatusCode::BAD_REQUEST)?;
    let target = if requested.is_empty() {
        root.clone()
    } else {
        let path = StdPath::new(requested);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    };
    let resolved = target
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(&target));
    if !resolved.starts_with(&root) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(resolved)
}

fn list_directory_entries(dir: &StdPath, offset: usize, limit: usize) -> (Vec<FsEntry>, usize) {
    let mut entries = Vec::new();
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path == dir {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        entries.push(FsEntry {
            name,
            path: path.display().to_string(),
            is_dir: path.is_dir(),
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let total = entries.len();
    let page = entries.into_iter().skip(offset).take(limit).collect();
    (page, total)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects",
    tag = "projects",
    responses((status = 200, description = "Recent projects", body = [Project]))
)]
pub(crate) async fn list_projects(State(state): State<AppState>) -> impl IntoResponse {
    let engine = state.engine.read().await;
    let store = ProjectsStore::new(engine.db.pool().clone());
    match store.list().await {
        Ok(projects) => (StatusCode::OK, Json(projects)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects",
    tag = "projects",
    request_body = CreateProjectRequest,
    responses((status = 200, description = "Project registered", body = Project))
)]
pub(crate) async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> impl IntoResponse {
    let engine = state.engine.read().await;
    let store = ProjectsStore::new(engine.db.pool().clone());
    match store.upsert(&req.path).await {
        Ok(project) => (StatusCode::OK, Json(project)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session",
    tag = "session",
    responses((status = 200, description = "List sessions", body = [Session]))
)]
pub(crate) async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.list().await {
        Ok(sessions) => (StatusCode::OK, Json(serde_json::json!(sessions))).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session",
    tag = "session",
    request_body = CreateSessionRequest,
    responses((status = 200, description = "Session created", body = Session))
)]
pub(crate) async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    if req.directory.is_empty() {
        req.directory = dir.clone();
    }
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.create(req).await {
        Ok(session) => (StatusCode::OK, Json(serde_json::json!(session))).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/prompt",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = PromptRequest,
    responses((status = 200, description = "Prompt admitted"))
)]
pub(crate) async fn prompt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut req): Json<PromptRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    req.session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.prompt(req).await {
        Ok(message_id) => (
            StatusCode::OK,
            Json(serde_json::json!({ "message_id": message_id.to_string() })),
        )
            .into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/interrupt",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session interrupted"))
)]
pub(crate) async fn interrupt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.interrupt(session_id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/active",
    tag = "session",
    responses((status = 200, description = "Active session IDs", body = [String]))
)]
pub(crate) async fn list_active_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.active().await {
        Ok(ids) => {
            let list: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
            Json(serde_json::json!(list)).into_response()
        }
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/move",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = MoveSessionRequest,
    responses((status = 200, description = "Session moved", body = crate::openapi::OkResponse))
)]
pub(crate) async fn move_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MoveSessionRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .move_session(session_id, req.directory)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/messages",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session messages", body = [Message]))
)]
pub(crate) async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.list_messages(session_id).await {
        Ok(messages) => Json(serde_json::json!(messages)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct TimelineQuery {
    #[serde(default)]
    since_seq: i64,
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/timeline",
    tag = "session",
    params(
        ("id" = String, Path, description = "Session ID"),
        ("since_seq" = i64, Query, description = "Return events after this sequence number")
    ),
    responses((status = 200, description = "Session timeline (deprecated — use /activity)", body = [crate::openapi::TimelineEntry]))
)]
#[deprecated(note = "Use GET /api/v1/session/{id}/activity instead")]
pub(crate) async fn list_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .list_timeline(session_id, query.since_seq)
        .await
    {
        Ok(timeline) => Json(serde_json::json!(timeline)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/activity",
    tag = "session",
    params(
        ("id" = String, Path, description = "Session ID"),
        ("since_seq" = i64, Query, description = "Return events after this sequence number")
    ),
    responses((status = 200, description = "Session activity tree", body = deepseekcode_schema::activity::ActivityTree))
)]
pub(crate) async fn list_activity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .list_activity(session_id, query.since_seq)
        .await
    {
        Ok(activity) => Json(activity).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/todo",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session todo list", body = [deepseekcode_schema::todo::TodoItem]))
)]
pub(crate) async fn get_session_todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location.session_service.get(session_id).await.is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response();
    }
    match location.session_service.get_todos(session_id).await {
        Ok(todos) => Json(todos).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/todo",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = deepseekcode_schema::todo::TodoUpdateRequest,
    responses((status = 200, description = "Updated todo list", body = [deepseekcode_schema::todo::TodoItem]))
)]
pub(crate) async fn update_session_todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<deepseekcode_schema::todo::TodoUpdateRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location.session_service.get(session_id).await.is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response();
    }
    match location
        .session_service
        .update_todos(session_id, &body.todos)
        .await
    {
        Ok(todos) => Json(todos).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/plans",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session plans", body = [deepseekcode_schema::plan::SessionPlanSummary]))
)]
pub(crate) async fn list_session_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location.session_service.get(session_id).await.is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response();
    }
    match location
        .session_service
        .list_session_plans(session_id)
        .await
    {
        Ok(plans) => Json(plans).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/plans/{plan_id}/dismiss",
    tag = "session",
    params(
        ("id" = String, Path, description = "Session ID"),
        ("plan_id" = String, Path, description = "Plan ID")
    ),
    responses((status = 200, description = "Dismissed plan implement prompt"))
)]
pub(crate) async fn dismiss_session_plan_prompt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, plan_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location.session_service.get(session_id).await.is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response();
    }
    match location
        .session_service
        .dismiss_plan_implement_prompt(session_id, &plan_id)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/events",
    tag = "session",
    params(
        ("id" = String, Path, description = "Session ID"),
        ("since_seq" = i64, Query, description = "Return events after this sequence number")
    ),
    responses((status = 200, description = "Session event log", body = [deepseekcode_schema::event::EventEnvelope]))
)]
pub(crate) async fn list_session_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .list_events(session_id, query.since_seq)
        .await
    {
        Ok(events) => Json(events).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct QuestionAnswerRequest {
    pub answer: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/question/answer",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = QuestionAnswerRequest,
    responses((status = 200, description = "Question answer submitted"))
)]
pub(crate) async fn answer_session_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<QuestionAnswerRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location.session_service.get(session_id).await.is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response();
    }
    match location
        .session_service
        .submit_question_answer(session_id, body.answer)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/events/head",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Latest persisted event sequence number"))
)]
pub(crate) async fn session_event_head(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.latest_event_seq(session_id).await {
        Ok(seq) => Json(serde_json::json!({ "seq": seq })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/compact",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session compacted", body = crate::openapi::OkResponse))
)]
pub(crate) async fn compact_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let registry = deepseekcode_core::system_context::SystemContextRegistry::new(dir.clone());
    let epochs =
        deepseekcode_core::system_context::ContextEpochStore::new(engine.db.pool().clone());
    match deepseekcode_core::compaction::CompactionService::compact(
        &registry,
        &epochs,
        &location.session_service,
        session_id,
        &engine.config,
    )
    .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/provider",
    tag = "provider",
    responses((status = 200, description = "Provider catalog", body = [ProviderInfo]))
)]
pub(crate) async fn list_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let providers = location.providers.list().await;
    Json(serde_json::json!(providers))
}

#[utoipa::path(
    post,
    path = "/api/v1/provider/{id}/connect",
    tag = "provider",
    params(("id" = String, Path, description = "Provider ID")),
    request_body = ConnectProviderRequest,
    responses((status = 200, description = "Provider connected", body = crate::openapi::OkResponse))
)]
pub(crate) async fn connect_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ConnectProviderRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let mut engine = state.engine.write().await;
    let location = engine.location(&dir).await;
    if let Some(key) = req.api_key.filter(|k| !k.is_empty()) {
        match location
            .providers
            .connect(&id, key, req.base_url.clone())
            .await
        {
            Ok(()) => {
                let _ = engine.reload_catalog(&dir).await;
                Json(serde_json::json!({ "ok": true })).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        Json(serde_json::json!({ "ok": true })).into_response()
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/provider/{id}/disconnect",
    tag = "provider",
    params(("id" = String, Path, description = "Provider ID")),
    responses((status = 200, description = "Provider disconnected", body = crate::openapi::OkResponse))
)]
pub(crate) async fn disconnect_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.providers.disconnect(&id).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/permission/pending",
    tag = "permission",
    responses((status = 200, description = "Pending permissions", body = [PermissionRequest]))
)]
pub(crate) async fn list_pending_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let pending = location.permissions.list_pending_legacy().await;
    Json(serde_json::json!(pending))
}

#[utoipa::path(
    post,
    path = "/api/v1/permission/{id}/respond",
    tag = "permission",
    params(("id" = String, Path, description = "Permission request ID")),
    request_body = PermissionResponse,
    responses((status = 200, description = "Permission response recorded", body = crate::openapi::OkResponse))
)]
pub(crate) async fn respond_permission(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PermissionResponse>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let permission_id = parse_permission_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .permissions
        .respond_legacy(permission_id, req.status)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/policy/pending",
    tag = "policy",
    responses((status = 200, description = "Pending policy prompts", body = [PolicyRequest]))
)]
pub(crate) async fn list_pending_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let pending = location.permissions.list_pending().await;
    Json(serde_json::json!(pending))
}

#[utoipa::path(
    post,
    path = "/api/v1/policy/{id}/reply",
    tag = "policy",
    params(("id" = String, Path, description = "Policy request ID")),
    request_body = PolicyReply,
    responses((status = 200, description = "Policy reply recorded", body = crate::openapi::OkResponse))
)]
pub(crate) async fn reply_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PolicyReply>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let permission_id = parse_permission_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.permissions.reply(permission_id, req).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/policy/grants",
    tag = "policy",
    responses((status = 200, description = "Project policy grants", body = [PolicyGrant]))
)]
pub(crate) async fn list_policy_grants(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.permissions.list_grants().await {
        Ok(grants) => Json(serde_json::json!(grants)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/policy/grants/{id}",
    tag = "policy",
    params(("id" = String, Path, description = "Grant ID")),
    responses((status = 200, description = "Grant deleted", body = crate::openapi::OkResponse))
)]
pub(crate) async fn delete_policy_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.permissions.delete_grant(&id).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct UpdatePolicyConfigRequest {
    pub policy: deepseekcode_config::PolicyConfig,
    #[serde(default)]
    pub policy_mode: Option<deepseekcode_config::PolicyMode>,
}

#[utoipa::path(
    put,
    path = "/api/v1/policy/config",
    tag = "policy",
    responses((status = 200, description = "Policy config saved", body = crate::openapi::OkResponse))
)]
pub(crate) async fn update_policy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpdatePolicyConfigRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let config_path = std::path::Path::new(&dir).join("deepseekcode.jsonc");
    let mut cfg = camino::Utf8Path::new(&dir)
        .canonicalize_utf8()
        .ok()
        .and_then(|p| deepseekcode_config::load_effective_config(&p).ok())
        .unwrap_or_default();
    cfg.policy = req.policy;
    if let Some(mode) = req.policy_mode {
        cfg.policy_mode = Some(mode);
    }
    let serialized = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if let Err(e) = std::fs::write(&config_path, serialized) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let mut engine = state.engine.write().await;
    match engine.reload_catalog(&dir).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/mcp",
    tag = "mcp",
    responses((status = 200, description = "MCP server status"))
)]
pub(crate) async fn list_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let status = location.mcp.status();
    Json(serde_json::json!(status))
}

#[utoipa::path(
    post,
    path = "/api/v1/mcp/{name}/connect",
    tag = "mcp",
    params(("name" = String, Path, description = "MCP server name")),
    responses((status = 200, description = "MCP server connected", body = crate::openapi::McpConnectResponse))
)]
pub(crate) async fn connect_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let mut engine = state.engine.write().await;
    let location = engine.location(&dir).await;
    match location.mcp.connect(&name).await {
        Ok(tools) => {
            let _ = engine.reload_catalog(&dir).await;
            Json(serde_json::json!({ "ok": true, "tools": tools })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mcp/{name}/disconnect",
    tag = "mcp",
    params(("name" = String, Path, description = "MCP server name")),
    responses((status = 200, description = "MCP server disconnected", body = crate::openapi::OkResponse))
)]
pub(crate) async fn disconnect_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let mut engine = state.engine.write().await;
    let location = engine.location(&dir).await;
    match location.mcp.disconnect(&name).await {
        Ok(()) => {
            let _ = engine.reload_catalog(&dir).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/skills",
    tag = "skills",
    responses((status = 200, description = "Discovered skills", body = [crate::openapi::SkillInfo]))
)]
pub(crate) async fn list_skills(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let mut engine = state.engine.write().await;
    let _ = engine.reload_catalog(&dir).await;
    let skills = engine
        .skills()
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "path": s.path.display().to_string(),
            })
        })
        .collect::<Vec<_>>();
    Json(serde_json::json!(skills))
}

#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    tag = "plugins",
    responses((status = 200, description = "Loaded plugins"))
)]
pub(crate) async fn list_plugins(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let mut engine = state.engine.write().await;
    let _ = engine.reload_catalog(&dir).await;
    let location = engine.location(&dir).await;
    let host = location.plugin_host.read().await;
    Json(serde_json::json!(host.list()))
}

#[derive(Deserialize)]
pub(crate) struct EventQuery {
    since_seq: Option<i64>,
    session_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/event",
    tag = "event",
    params(
        ("since_seq" = Option<i64>, Query, description = "Replay events after this sequence"),
        ("session_id" = Option<String>, Query, description = "Filter events to a session"),
    ),
    responses((status = 200, description = "SSE event stream"))
)]
pub(crate) async fn event_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let store = location.session_service.event_store();
    let since = query.since_seq.unwrap_or(0);
    let filter_session = query
        .session_id
        .as_deref()
        .and_then(|id| SessionId::from_str(id).ok());

    let live_rx = store.subscribe();
    let handoff_seq = store.latest_head_seq().await.unwrap_or(0);

    let history = store
        .tail_filtered(since, sse_replay_limit(), filter_session)
        .await
        .unwrap_or_default();

    let replay_limit = sse_replay_limit();
    let stream: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if history.len() as i64 >= replay_limit {
            Box::pin(tokio_stream::once(Ok(Event::default()
                .json_data(serde_json::json!({
                    "error": "gap_too_large",
                    "head": handoff_seq,
                    "limit": replay_limit,
                }))
                .unwrap_or_else(|_| Event::default()))))
        } else {
            let history_stream = tokio_stream::iter(
                history
                    .into_iter()
                    .filter(move |e| e.seq <= handoff_seq)
                    .map(|e| {
                        Ok(Event::default()
                            .json_data(e)
                            .unwrap_or_else(|_| Event::default()))
                    }),
            );
            let live_handoff = handoff_seq;
            let live = BroadcastStream::new(live_rx)
                .filter_map(|r| r.ok())
                .filter(move |e| {
                    e.seq > live_handoff
                        && filter_session
                            .map(|sid| e.session_id == Some(sid))
                            .unwrap_or(true)
                })
                .map(|e| {
                    Ok(Event::default()
                        .json_data(e)
                        .unwrap_or_else(|_| Event::default()))
                });
            Box::pin(history_stream.chain(live))
        };

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Deserialize)]
pub(crate) struct FsTreeQuery {
    path: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/api/v1/fs/tree",
    tag = "fs",
    params(
        ("path" = Option<String>, Query, description = "Directory to list (defaults to project root)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset"),
        ("limit" = Option<usize>, Query, description = "Pagination limit (default 500)"),
    ),
    responses((status = 200, description = "Directory entries", body = FsTreeResponse))
)]
pub(crate) async fn fs_tree(
    headers: HeaderMap,
    Query(query): Query<FsTreeQuery>,
) -> impl IntoResponse {
    let project_root = project_dir(&headers);
    let requested = query.path.unwrap_or_default();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(500).min(1000);

    let dir = match resolve_within_project(&project_root, &requested) {
        Ok(path) => path,
        Err(status) => {
            return (
                status,
                Json(serde_json::json!({ "error": "path outside project root" })),
            )
                .into_response();
        }
    };

    if !dir.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "not a directory" })),
        )
            .into_response();
    }

    let (entries, total) = list_directory_entries(&dir, offset, limit);
    Json(FsTreeResponse {
        entries,
        total,
        offset,
        limit,
    })
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct FsFileQuery {
    path: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/fs/file",
    tag = "fs",
    params(("path" = String, Query, description = "File path within project root")),
    responses((status = 200, description = "File contents", body = FsFileResponse))
)]
pub(crate) async fn fs_file(
    headers: HeaderMap,
    Query(query): Query<FsFileQuery>,
) -> impl IntoResponse {
    let project_root = project_dir(&headers);
    let path = match resolve_within_project(&project_root, &query.path) {
        Ok(path) => path,
        Err(status) => {
            return (
                status,
                Json(serde_json::json!({ "error": "path outside project root" })),
            )
                .into_response();
        }
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => Json(serde_json::json!({ "content": content })).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/fs/file",
    tag = "fs",
    request_body = deepseekcode_schema::project::FsWriteRequest,
    responses(
        (status = 200, description = "File written"),
        (status = 400, description = "Missing or invalid x-deepseekcode-session-id header"),
        (status = 403, description = "Direct writes disabled or policy denied"),
    )
)]
pub(crate) async fn fs_write(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<deepseekcode_schema::project::FsWriteRequest>,
) -> impl IntoResponse {
    if !fs_write_allowed() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "direct file writes are disabled; use agent tools or enable DEEPSEEKCODE_ALLOW_FS_WRITE=1"
            })),
        )
            .into_response();
    }
    let session_id = match headers
        .get("x-deepseekcode-session-id")
        .and_then(|v| v.to_str().ok())
    {
        Some(raw) => match SessionId::from_str(raw) {
            Ok(id) => id,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid x-deepseekcode-session-id header"
                    })),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "x-deepseekcode-session-id header required for fs_write"
                })),
            )
                .into_response();
        }
    };
    let project_root = project_dir(&headers);
    let dir = project_root.clone();
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    if location
        .session_service
        .ensure_session_in_location(session_id)
        .await
        .is_err()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found in this project" })),
        )
            .into_response();
    }
    let input = serde_json::json!({
        "path": body.path,
        "content": body.content,
    });
    if let Err(e) = location
        .permissions
        .ensure_allowed(
            session_id,
            "write",
            &deepseekcode_schema::agent::AgentKind::Build,
            input,
        )
        .await
    {
        return service_error_response(e).into_response();
    }
    let path = match resolve_within_project(&project_root, &body.path) {
        Ok(path) => path,
        Err(status) => {
            return (
                status,
                Json(serde_json::json!({ "error": "path outside project root" })),
            )
                .into_response();
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }
    match std::fs::write(&path, &body.content) {
        Ok(()) => {
            tracing::info!(
                target: deepseekcode_telemetry::SERVER,
                path = %body.path,
                session_id = %session_id,
                "fs_write via api"
            );
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct DiffQuery {
    path: String,
    original: Option<String>,
    snapshot_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/fs/diff",
    tag = "fs",
    params(
        ("path" = String, Query, description = "Current file path"),
        ("original" = Option<String>, Query, description = "Original file path for comparison"),
        ("snapshot_id" = Option<String>, Query, description = "Revert snapshot id for original content"),
    ),
    responses((status = 200, description = "Unified diff", body = FsDiffResponse))
)]
pub(crate) async fn fs_diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DiffQuery>,
) -> impl IntoResponse {
    let project_root = project_dir(&headers);
    let path = match resolve_within_project(&project_root, &query.path) {
        Ok(path) => path,
        Err(status) => {
            return (
                status,
                Json(serde_json::json!({ "error": "path outside project root" })),
            )
                .into_response();
        }
    };

    let original = if let Some(snapshot_id) = query.snapshot_id.as_deref() {
        let engine = state.engine.read().await;
        let store = deepseekcode_core::FileSnapshotStore::new(
            engine.db.pool().clone(),
            &project_root,
            true,
        );
        match store.get(snapshot_id).await {
            Ok(Some(snap)) => std::fs::read_to_string(&snap.snapshot_path).unwrap_or_default(),
            _ => String::new(),
        }
    } else if let Some(original_path) = query.original.as_deref() {
        match resolve_within_project(&project_root, original_path) {
            Ok(path) => std::fs::read_to_string(path).unwrap_or_default(),
            Err(_) => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({ "error": "original path outside project root" })),
                )
                    .into_response();
            }
        }
    } else {
        String::new()
    };

    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let diff = similar::TextDiff::from_lines(&original, &current);
    let hunks = diff.unified_diff().context_radius(3).to_string();
    Json(serde_json::json!({ "hunks": hunks })).into_response()
}

#[utoipa::path(
    post,
    path = "/api/v1/pty",
    tag = "pty",
    request_body = CreatePtyRequest,
    responses((status = 200, description = "PTY created", body = PtyCreateResponse))
)]
pub(crate) async fn create_pty(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreatePtyRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    match state
        .pty
        .create(&dir, req.rows.unwrap_or(24), req.cols.unwrap_or(80))
    {
        Ok(session) => Json(serde_json::json!({ "id": session.id.to_string() })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(Deserialize, serde::Serialize, utoipa::ToSchema)]
pub(crate) struct ResizePtyRequest {
    rows: u16,
    cols: u16,
}

#[utoipa::path(
    post,
    path = "/api/v1/pty/{id}/resize",
    tag = "pty",
    params(("id" = String, Path, description = "PTY session id")),
    request_body = ResizePtyRequest,
    responses((status = 200, description = "PTY resized"))
)]
pub(crate) async fn resize_pty(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ResizePtyRequest>,
) -> impl IntoResponse {
    let _dir = project_dir(&headers);
    match state.pty.resize(&id, req.rows, req.cols) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/pty/{id}/ticket",
    tag = "pty",
    params(("id" = String, Path, description = "PTY session id")),
    responses((status = 200, description = "WebSocket connect ticket", body = PtyTicketResponse))
)]
pub(crate) async fn pty_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.password.is_some()
        && headers.get(PTY_TICKET_HEADER).and_then(|v| v.to_str().ok())
            != Some(PTY_TICKET_HEADER_VALUE)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "missing PTY ticket header" })),
        )
            .into_response();
    }

    let dir = project_dir(&headers);
    match state.pty.issue_ticket(&id, &dir) {
        Ok((ticket, expires_in)) => Json(PtyTicketResponse { ticket, expires_in }).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/pty/{id}/connect",
    tag = "pty",
    params(
        ("id" = String, Path, description = "PTY session id"),
        ("ticket" = Option<String>, Query, description = "One-time connect ticket when server password is set"),
    ),
    responses(
        (status = 101, description = "WebSocket upgrade for PTY I/O"),
        (status = 403, description = "Invalid or expired ticket")
    )
)]
pub(crate) async fn pty_connect(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PtyConnectQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    if state.password.is_some() {
        let ticket = query.ticket.as_deref().unwrap_or_default();
        if !state.pty.consume_ticket(ticket, &id, &dir) {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "invalid or expired PTY ticket" })),
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_pty_ws(socket, state, id))
}

#[derive(Deserialize)]
pub(crate) struct PtyConnectQuery {
    ticket: Option<String>,
}

async fn handle_pty_ws(mut socket: WebSocket, state: AppState, id: String) {
    let Some(session) = state.pty.get(&id) else {
        return;
    };
    let mut rx = session.input_tx.subscribe();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(WsMessage::Binary(data))) => {
                        let _ = state.pty.write(&id, &data);
                    }
                    Some(Ok(WsMessage::Text(text))) => {
                        let _ = state.pty.write(&id, text.as_bytes());
                    }
                    _ => break,
                }
            }
            output = rx.recv() => {
                if let Ok(data) = output {
                    if socket.send(WsMessage::Binary(data.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session", body = Session))
)]
pub(crate) async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.get(session_id).await {
        Ok(session) => Json(session).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/session/{id}",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = UpdateSessionRequest,
    responses((status = 200, description = "Session updated", body = crate::openapi::OkResponse))
)]
pub(crate) async fn update_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateSessionRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .update_session(session_id, req.title)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/session/{id}",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session deleted", body = crate::openapi::OkResponse))
)]
pub(crate) async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.delete(session_id).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/model",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = UpdateModelRequest,
    responses((status = 200, description = "Session model updated", body = crate::openapi::OkResponse))
)]
pub(crate) async fn update_session_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateModelRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let agent = match req.agent.as_deref() {
        Some("plan") => deepseekcode_schema::agent::AgentKind::Plan,
        Some("general") => deepseekcode_schema::agent::AgentKind::General,
        _ => deepseekcode_schema::agent::AgentKind::Build,
    };
    let mut engine = state.engine.write().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .update_model(session_id, agent, req.model_id, req.provider_id)
        .await
    {
        Ok(()) => {
            let _ = engine.reload_catalog(&dir).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/fork",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = ForkRequest,
    responses((status = 200, description = "Session forked", body = Session))
)]
pub(crate) async fn fork_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<ForkRequest>>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let until_message_id = body.and_then(|Json(req)| req.until_message_id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .fork(session_id, until_message_id)
        .await
    {
        Ok(new_session) => Json(serde_json::json!(new_session)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/revert",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Staged revert state", body = RevertState))
)]
pub(crate) async fn revert_get_staged(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.revert_get_staged(session_id).await {
        Ok(Some(revert)) => Json(serde_json::json!(revert)).into_response(),
        Ok(None) => Json(serde_json::Value::Null).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/revert/stage",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    request_body = RevertStageRequest,
    responses((status = 200, description = "Revert staged", body = RevertState))
)]
pub(crate) async fn revert_stage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RevertStageRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .revert_stage(session_id, req.message_id)
        .await
    {
        Ok(revert) => Json(serde_json::json!(revert)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/revert/clear",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Staged revert cleared", body = crate::openapi::OkResponse))
)]
pub(crate) async fn revert_clear(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.revert_clear(session_id).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/session/{id}/revert/commit",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Revert committed", body = RevertState))
)]
pub(crate) async fn revert_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.revert_commit(session_id).await {
        Ok(revert) => Json(serde_json::json!(revert)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ExportQuery {
    #[serde(default = "default_true")]
    sanitize: bool,
}

fn default_true() -> bool {
    true
}

#[utoipa::path(
    get,
    path = "/api/v1/session/{id}/export",
    tag = "session",
    params(
        ("id" = String, Path, description = "Session ID"),
        ("sanitize" = Option<bool>, Query, description = "Redact sensitive fields from export"),
    ),
    responses((status = 200, description = "Session export", body = crate::openapi::SessionExportResponse))
)]
pub(crate) async fn export_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location
        .session_service
        .export_session(session_id, query.sanitize)
        .await
    {
        Ok(export) => Json(export).into_response(),
        Err(e) => {
            tracing::error!(
                target: deepseekcode_telemetry::SERVER,
                error = %e,
                session_id = %id,
                "export session failed"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct DiagnosticsQuery {
    run_id: Option<String>,
    #[serde(default = "default_true")]
    redact: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/diagnostics/export",
    tag = "diagnostics",
    params(("run_id" = Option<String>, Query, description = "Optional diagnostic run id")),
    responses(
        (status = 200, description = "Diagnostic bundle (application/zip)", content_type = "application/zip"),
        (status = 500, description = "Export failed")
    )
)]
pub(crate) async fn export_diagnostics(Query(query): Query<DiagnosticsQuery>) -> impl IntoResponse {
    let temp = std::env::temp_dir().join(format!(
        "{DIAGNOSTICS_ARCHIVE_PREFIX}-{}.zip",
        ulid::Ulid::new()
    ));
    match deepseekcode_telemetry::export_diagnostic_bundle_with_options(
        query.run_id.as_deref(),
        &temp,
        query.redact,
    ) {
        Ok(path) => match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let _ = tokio::fs::remove_file(&path).await;
                (
                    StatusCode::OK,
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/zip"),
                    )],
                    bytes,
                )
                    .into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/project/directories",
    tag = "worktree",
    responses((status = 200, description = "Project directories", body = [crate::openapi::ProjectDirectoryEntry]))
)]
pub(crate) async fn list_project_directories(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let store = deepseekcode_core::git::ProjectDirectoryStore::new(
        state.engine.read().await.db.pool().clone(),
    );
    match store.list(&dir).await {
        Ok(entries) => Json(serde_json::json!(entries)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/project/directories",
    tag = "worktree",
    request_body = AddProjectDirectoryRequest,
    responses((status = 200, description = "Project directory added", body = crate::openapi::ProjectDirectoryEntry))
)]
pub(crate) async fn add_project_directory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AddProjectDirectoryRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let store = deepseekcode_core::git::ProjectDirectoryStore::new(
        state.engine.read().await.db.pool().clone(),
    );
    match store
        .add(&dir, &req.directory_path, req.strategy.as_deref())
        .await
    {
        Ok(entry) => Json(serde_json::json!(entry)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/worktree",
    tag = "worktree",
    responses((status = 200, description = "Git worktrees", body = [crate::openapi::WorktreeEntry]))
)]
pub(crate) async fn list_worktrees(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let manager = deepseekcode_core::git::GitWorktreeManager::new(&dir);
    if !manager.is_git_repo() {
        return Json(serde_json::json!([])).into_response();
    }
    match manager.list() {
        Ok(entries) => Json(serde_json::json!(entries)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/worktree",
    tag = "worktree",
    request_body = CreateWorktreeRequest,
    responses((status = 200, description = "Worktree created", body = crate::openapi::WorktreeEntry))
)]
pub(crate) async fn create_worktree(
    State(_state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateWorktreeRequest>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let manager = deepseekcode_core::git::GitWorktreeManager::new(&dir);
    match manager.create(req.branch.as_deref()) {
        Ok(entry) => Json(serde_json::json!(entry)).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct RemoveWorktreeQuery {
    path: String,
}

#[utoipa::path(
    delete,
    path = "/api/v1/worktree",
    tag = "worktree",
    params(("path" = String, Query, description = "Worktree path to remove")),
    responses((status = 200, description = "Worktree removed", body = crate::openapi::OkResponse))
)]
pub(crate) async fn remove_worktree(
    State(_state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RemoveWorktreeQuery>,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let manager = deepseekcode_core::git::GitWorktreeManager::new(&dir);
    match manager.remove(&query.path) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/worktree/prune",
    tag = "worktree",
    responses((status = 200, description = "Stale worktree metadata pruned", body = crate::openapi::OkResponse))
)]
pub(crate) async fn prune_worktrees(
    State(_state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let manager = deepseekcode_core::git::GitWorktreeManager::new(&dir);
    if !manager.is_git_repo() {
        return Json(serde_json::json!({ "ok": true })).into_response();
    }
    match manager.prune() {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}
