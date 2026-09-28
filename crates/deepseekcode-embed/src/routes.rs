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
    routing::{delete, get, post, put},
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
use deepseekcode_storage::{normalize_directory_path, ProjectsStore};
use deepseekcode_types::{SessionId, DIAGNOSTICS_ARCHIVE_PREFIX};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::{Component, Path as StdPath, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use crate::openapi;
use deepseekcode_schema::api::{
    AddProjectDirectoryRequest, CreatePtyRequest, CreateWorktreeRequest, ForkRequest,
    MoveSessionRequest, UpdateModelRequest,
};

use crate::state::AppState;

const SSE_REPLAY_LIMIT: i64 = 10_000;

fn sse_replay_limit() -> i64 {
    std::env::var("DEEPSEEKCODE_SSE_REPLAY_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .map(|n: i64| n.min(SSE_REPLAY_LIMIT))
        .unwrap_or(SSE_REPLAY_LIMIT)
}

/// Directories a client header may address. `DEEPSEEKCODE_ALLOWED_ROOTS` is an
/// opt-in `;`-separated allowlist; entries are canonicalized once per request.
/// Unset/empty means "no restriction" (back-compat: the server is
/// location-scoped per directory by design, and localhost binding + password
/// remain the primary boundary per SECURITY.md). Deployments exposed beyond
/// localhost MUST set `DEEPSEEKCODE_ALLOWED_ROOTS` to the served tree.
fn allowed_roots() -> Option<Vec<PathBuf>> {
    let raw = std::env::var("DEEPSEEKCODE_ALLOWED_ROOTS").unwrap_or_default();
    if raw.trim().is_empty() {
        return None;
    }
    let mut roots = Vec::new();
    for part in raw.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        roots.push(canonical_lossy(StdPath::new(part)));
    }
    Some(roots)
}

fn canonical_lossy(p: &StdPath) -> PathBuf {
    std::fs::canonicalize(p)
        .map(|c| strip_extended_path_prefix(&c))
        .unwrap_or_else(|_| normalize_path(p))
}

fn is_dir_allowed(dir: &str) -> bool {
    let Some(roots) = allowed_roots() else {
        return true;
    };
    let candidate = canonical_lossy(StdPath::new(dir));
    roots
        .iter()
        .any(|root| candidate == *root || candidate.starts_with(root))
}

fn validated_project_root(
    headers: &HeaderMap,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let dir = project_dir(headers);
    if !is_dir_allowed(&dir) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "directory not allowed", "code": "forbidden" })),
        ));
    }
    Ok(dir)
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
    use deepseekcode_types::DeepSeekCodeError as E;
    let status = match &e {
        E::NotFound(_) => StatusCode::NOT_FOUND,
        E::PermissionDenied(_) => StatusCode::FORBIDDEN,
        E::InvalidInput(_) => StatusCode::BAD_REQUEST,
        E::AlreadyExists(_) => StatusCode::CONFLICT,
        E::SessionInterrupted => StatusCode::CONFLICT,
        E::ContextUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        E::RetryAfterCompaction | E::RetryWithoutReasoning => StatusCode::SERVICE_UNAVAILABLE,
        E::Llm(msg) => {
            if deepseekcode_llm::is_context_overflow(msg) {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                let lower = msg.to_ascii_lowercase();
                if lower.contains("unauthorized") || lower.contains("invalid api key") {
                    StatusCode::UNAUTHORIZED
                } else if lower.contains("rate limit") || lower.contains("too many requests") {
                    StatusCode::TOO_MANY_REQUESTS
                } else {
                    StatusCode::BAD_GATEWAY
                }
            }
        }
        E::ToolFailure(_) => StatusCode::UNPROCESSABLE_ENTITY,
        E::Config(_) => StatusCode::BAD_REQUEST,
        E::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    // Never leak storage internals (table names / SQL). Cap message length.
    let message = match &e {
        E::Storage(_) => "storage error".to_string(),
        other => {
            let s = other.to_string();
            if s.len() > 500 {
                s[..500].to_string()
            } else {
                s
            }
        }
    };
    // Single canonical shape: { error, code }.
    let code = match &e {
        E::NotFound(_) => "not_found",
        E::PermissionDenied(_) => "forbidden",
        E::InvalidInput(_) => "invalid_input",
        E::AlreadyExists(_) => "conflict",
        E::SessionInterrupted => "interrupted",
        E::ContextUnavailable(_) => "context_unavailable",
        E::RetryAfterCompaction | E::RetryWithoutReasoning => "retryable",
        E::Llm(_) => "llm_error",
        E::ToolFailure(_) => "tool_failure",
        E::Config(_) => "config_error",
        E::Storage(_) => "storage_error",
        _ => "internal",
    };
    (
        status,
        Json(serde_json::json!({ "error": message, "code": code })),
    )
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
    // Back-compat: full router including public health routes.
    authed_router(state.clone())
        .route("/health", get(health_root))
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi_json))
}

/// Routes that REQUIRE auth (everything except health/openapi).
/// Used by `app::build_router` to keep probes public.
pub fn authed_router(state: AppState) -> Router {
    Router::new()
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
        // Static `/active` must precede `/{id}` so `GET /active` is not
        // captured as `id="active"` (400 from parse_session_id).
        .route("/api/v1/session/active", get(list_active_sessions))
        .route(
            "/api/v1/session/{id}",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route("/api/v1/session/{id}/prompt", post(prompt))
        .route("/api/v1/session/{id}/interrupt", post(interrupt))
        .route("/api/v1/session/{id}/messages", get(list_messages))
        .route("/api/v1/session/{id}/timeline", {
            #[allow(deprecated)]
            get(list_timeline)
        })
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
        .route(
            "/api/v1/session/{id}/cache-status",
            get(session_cache_status),
        )
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
        // Legacy thin wrappers for policy grants/config (spec: remain for one
        // release cycle; removed in v0.3.0). Dedicated handlers so the
        // `Sunset` marker lands only on the legacy paths.
        .route("/api/v1/permission/grants", get(legacy_list_policy_grants))
        .route(
            "/api/v1/permission/grants/{id}",
            delete(legacy_delete_policy_grant),
        )
        .route(
            "/api/v1/permission/config",
            put(legacy_update_policy_config),
        )
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
        .route("/api/v1/pty/{id}", delete(destroy_pty))
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
    // Cap header length and reject NUL to bound DoS / log injection.
    let truncated: String = raw.chars().take(1024).collect();
    let cleaned = truncated.replace('\0', "");
    normalize_server_directory(&cleaned)
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
            .map(|p| normalize_directory_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| ".".into());
    }
    normalize_directory_path(path)
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

fn strip_extended_path_prefix(path: &StdPath) -> PathBuf {
    let rendered = path.to_string_lossy();
    // Handle `\\?\`, `\\.\`, and UNC (`\\server\share`) verbatim prefixes.
    for prefix in [r"\\?\", r"\\.\"] {
        if let Some(stripped) = rendered.strip_prefix(prefix) {
            return PathBuf::from(stripped);
        }
    }
    if rendered.starts_with(r"\\") {
        // UNC: normalize slashes for stable comparison.
        return PathBuf::from(rendered.replace('/', "\\"));
    }
    path.to_path_buf()
}

fn resolve_within_project(project_root: &str, requested: &str) -> Result<PathBuf, StatusCode> {
    if requested.contains('\0') || project_root.contains('\0') {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Reject absolute `requested` outside the root up front: only join
    // relative paths; absolute inputs must already be under root.
    let root = strip_extended_path_prefix(
        &std::fs::canonicalize(project_root).map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    let target = if requested.is_empty() {
        root.clone()
    } else {
        let requested_path = strip_extended_path_prefix(StdPath::new(requested));
        if requested_path.is_absolute() {
            // Absolute path must already be inside root after canonicalization.
            let canon = requested_path
                .canonicalize()
                .map(|p| strip_extended_path_prefix(&p))
                .unwrap_or_else(|_| normalize_path(&requested_path));
            if canon != root && !canon.starts_with(&root) {
                return Err(StatusCode::FORBIDDEN);
            }
            canon
        } else {
            root.join(requested_path)
        }
    };
    // Follow symlinks (canonicalize) so `link -> /etc` escapes are blocked.
    // For not-yet-existing files, canonicalize the existing parent prefix.
    let resolved = target
        .canonicalize()
        .map(|p| strip_extended_path_prefix(&p))
        .unwrap_or_else(|_| {
            if let Some(parent) = target.parent() {
                if let Ok(canonical_parent) = parent.canonicalize() {
                    let parent = strip_extended_path_prefix(&canonical_parent);
                    if let Some(file_name) = target.file_name() {
                        // Reject `..` / absolute file names that escape.
                        let name = file_name.to_string_lossy();
                        if name.contains('\0') || name == ".." {
                            return root.join("__rejected__");
                        }
                        return parent.join(file_name);
                    }
                    return parent;
                }
            }
            normalize_path(&target)
        });
    let resolved = strip_extended_path_prefix(&resolved);
    // Re-canonicalize once more to close check-then-use symlink races on
    // the parent; callers must open with `O_NOFOLLOW`-style handling where
    // available, but this blocks the common escape.
    if resolved != root && !resolved.starts_with(&root) {
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
    let path_id = parse_session_path!(&id);
    // Do not silently mask client mistakes: body session_id must match path.
    if req.session_id != path_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "body session_id does not match path id" })),
        )
            .into_response();
    }
    req.session_id = path_id;
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
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
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
        Err(e) => service_error_response(e).into_response(),
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
    get,
    path = "/api/v1/session/{id}/cache-status",
    tag = "session",
    params(("id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "DeepSeek prefix-cache status"))
)]
pub(crate) async fn session_cache_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    use deepseekcode_schema::event::EventKind;
    let dir = project_dir(&headers);
    let session_id = parse_session_path!(&id);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    match location.session_service.list_events(session_id, 0).await {
        Ok(events) => {
            let mut payloads = Vec::new();
            let mut compacted = Vec::new();
            for e in events {
                match e.kind {
                    EventKind::MessageCompleted => payloads.push(e.payload),
                    EventKind::SessionCompacted => compacted.push(e.payload),
                    _ => {}
                }
            }
            let status =
                deepseekcode_core::cache_status::aggregate_with_compaction(&payloads, &compacted);
            // P1-5: read-only cost attribution from the configured rate
            // card. Never gates requests; purely so "tokens saved" is
            // auditable as money saved.
            let card = deepseekcode_core::cache_status::RateCard::from(&engine.config.billing);
            let estimated_cost = deepseekcode_core::cache_status::estimated_cost(&status, &card);
            Json(serde_json::json!({
                "session_id": id,
                "turns": status.turns,
                "input_tokens": status.input_tokens,
                "output_tokens": status.output_tokens,
                "reasoning_tokens": status.reasoning_tokens,
                "cache_hit": status.cache_hit,
                "cache_miss": status.cache_miss,
                "cache_write": status.cache_write,
                "hit_rate": status.hit_rate,
                "receipt": status.receipt(),
                "summary_count": status.summary_count,
                "summary_input_tokens": status.summary_input_tokens,
                "summary_output_tokens": status.summary_output_tokens,
                "summary_cache_hit": status.summary_cache_hit,
                "summary_cache_miss": status.summary_cache_miss,
                "summary_cache_write": status.summary_cache_write,
                "first_input_tokens": status.first_input_tokens,
                "first_cache_hit": status.first_cache_hit,
                "first_cache_miss": status.first_cache_miss,
                "first_hit_rate": status.first_hit_rate,
                "currency": card.currency,
                "estimated_cost": estimated_cost,
            }))
            .into_response()
        }
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
    // P0-2: manual compaction runs through the live runner so the checkpoint
    // carries the real baseline/messages/tools lineage (never a hollow
    // context that would poison the next auto pass).
    match location.session_service.compact_manual(session_id).await {
        Ok(installed) => Json(serde_json::json!({ "ok": installed })).into_response(),
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
    // Read-only catalog access: do not hold engine.write() across network IO.
    let location = {
        let engine = state.engine.read().await;
        engine.location(&dir).await
    };
    if let Some(key) = req.api_key.filter(|k| !k.trim().is_empty()) {
        match location
            .providers
            .connect(&id, key, req.base_url.clone())
            .await
        {
            Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
            Err(e) => service_error_response(e).into_response(),
        }
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "api_key is required" })),
        )
            .into_response()
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
        Err(e) => service_error_response(e).into_response(),
    }
}

/// Attach the legacy-compat `Sunset` marker: `permission/*` are thin
/// wrappers kept for one release cycle per `specs/policy-system.md` and are
/// removed in v0.3.0 (callers: desktop and CLI already use `policy/*`).
fn legacy_sunset(response: impl IntoResponse) -> axum::response::Response {
    const REMOVAL: &str = "v0.3.0; use /api/v1/policy/* instead";
    let mut response = response.into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(REMOVAL) {
        response.headers_mut().insert("sunset", value);
    }
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/permission/pending",
    tag = "permission",
    responses((status = 200, description = "Pending permissions (DEPRECATED: use /api/v1/policy/pending; removed in v0.3.0)", body = [PermissionRequest]))
)]
pub(crate) async fn list_pending_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let dir = project_dir(&headers);
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    let pending = location.permissions.list_pending_legacy().await;
    legacy_sunset(Json(serde_json::json!(pending)).into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/permission/{id}/respond",
    tag = "permission",
    params(("id" = String, Path, description = "Permission request ID")),
    request_body = PermissionResponse,
    responses((status = 200, description = "Permission response recorded (DEPRECATED: use /api/v1/policy/{id}/reply; removed in v0.3.0)", body = crate::openapi::OkResponse))
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
    let response = match location
        .permissions
        .respond_legacy(permission_id, req.status)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    };
    legacy_sunset(response)
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
        Err(e) => service_error_response(e).into_response(),
    }
}

/// Legacy `permission/*` wrappers over the policy handlers. Each delegates
/// and stamps the `Sunset` marker; removed in v0.3.0 with the legacy
/// `permissions` table (see `PermissionStore` deprecation).
#[utoipa::path(
    get,
    path = "/api/v1/permission/grants",
    tag = "permission",
    responses((status = 200, description = "Project policy grants (DEPRECATED: use /api/v1/policy/grants; removed in v0.3.0)", body = [PolicyGrant]))
)]
pub(crate) async fn legacy_list_policy_grants(
    state: State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    legacy_sunset(list_policy_grants(state, headers).await.into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/permission/grants/{id}",
    tag = "permission",
    params(("id" = String, Path, description = "Grant ID")),
    responses((status = 200, description = "Grant deleted (DEPRECATED: use /api/v1/policy/grants/{id}; removed in v0.3.0)", body = crate::openapi::OkResponse))
)]
pub(crate) async fn legacy_delete_policy_grant(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
) -> impl IntoResponse {
    legacy_sunset(
        delete_policy_grant(state, headers, path)
            .await
            .into_response(),
    )
}

#[utoipa::path(
    put,
    path = "/api/v1/permission/config",
    tag = "permission",
    responses((status = 200, description = "Policy config saved (DEPRECATED: use /api/v1/policy/config; removed in v0.3.0)", body = crate::openapi::OkResponse))
)]
pub(crate) async fn legacy_update_policy_config(
    state: State<AppState>,
    headers: HeaderMap,
    body: axum::Json<UpdatePolicyConfigRequest>,
) -> impl IntoResponse {
    legacy_sunset(
        update_policy_config(state, headers, body)
            .await
            .into_response(),
    )
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
    if !is_dir_allowed(&dir) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "directory not allowed", "code": "forbidden" })),
        )
            .into_response();
    }
    // Project-scoped delete via the store so enumerable IDs cannot delete
    // another project's grants.
    let engine = state.engine.read().await;
    let pool = engine.db.pool().clone();
    let store = deepseekcode_storage::PolicyStore::new(pool);
    match store.delete_grant_in_project(&id, &dir).await {
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
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    // Persist ONLY the policy delta to the project file: previously the full
    // effective config (merged parents+defaults) was pretty-printed over
    // deepseekcode.jsonc, flattening hierarchy and dropping JSONC comments.
    // NOTE: writing pretty JSON still drops comments; callers should keep a
    // backup. Atomic rename below avoids half-written files.
    let canonical_dir = canonical_lossy(StdPath::new(&dir));
    let config_path = canonical_dir.join("deepseekcode.jsonc");
    let existing: serde_json::Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| {
            // Preserve comments: parse JSONC, fall back to empty object.
            jsonc_parser::parse_to_serde_value(&s, &jsonc_parser::ParseOptions::default())
                .ok()
                .flatten()
                .or_else(|| serde_json::from_str(&s).ok())
        })
        .unwrap_or_else(|| serde_json::json!({}));
    let mut obj = existing.as_object().cloned().unwrap_or_default();
    obj.insert(
        "policy".into(),
        serde_json::to_value(&req.policy).unwrap_or_else(|_| serde_json::json!({})),
    );
    if let Some(mode) = req.policy_mode {
        let mode_str = match mode {
            deepseekcode_config::PolicyMode::Auto => "auto",
            deepseekcode_config::PolicyMode::Interactive => "interactive",
        };
        obj.insert(
            "policy_mode".into(),
            serde_json::Value::String(mode_str.into()),
        );
    }
    let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .unwrap_or_else(|_| "{}".into());
    // Atomic write (tmp + rename) without holding engine.write() across IO.
    let tmp_path = canonical_dir.join(format!(".deepseekcode.jsonc.tmp{}", std::process::id()));
    if let Err(_e) = std::fs::write(&tmp_path, serialized).and_then(|()| {
        std::fs::rename(&tmp_path, &config_path)
            .map_err(|e| std::io::Error::new(e.kind(), e.to_string()))
    }) {
        let _ = std::fs::remove_file(&tmp_path);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to persist policy config", "code": "storage_error" })),
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
    let location = {
        let engine = state.engine.read().await;
        engine.location(&dir).await
    };
    match location.mcp.connect(&name).await {
        Ok(tools) => Json(serde_json::json!({ "ok": true, "tools": tools })).into_response(),
        Err(e) => service_error_response(e).into_response(),
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
    let location = {
        let engine = state.engine.read().await;
        engine.location(&dir).await
    };
    match location.mcp.disconnect(&name).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
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
    let _dir = project_dir(&headers);
    // Read-only: never take engine.write() or reload on a list call
    // (previously serialized the whole server per list).
    let engine = state.engine.read().await;
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
    let engine = state.engine.read().await;
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
) -> impl IntoResponse {
    fn error_sse(msg: serde_json::Value) -> impl IntoResponse {
        let err = tokio_stream::once::<Result<Event, Infallible>>(Ok(Event::default()
            .json_data(msg)
            .unwrap_or_else(|_| Event::default())));
        Sse::new(err).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
    }
    let dir = project_dir(&headers);
    // Enforce allowlist even for SSE: forbidden dirs get an error-only stream.
    if !is_dir_allowed(&dir) {
        return error_sse(serde_json::json!({ "error": "forbidden", "code": "forbidden" }))
            .into_response();
    }
    let engine = state.engine.read().await;
    let location = engine.location(&dir).await;
    // Cross-project subscription guard: if a session filter is supplied it
    // must belong to this location, otherwise emit error-only stream.
    let mut filter_session = query
        .session_id
        .as_deref()
        .and_then(|id| SessionId::from_str(id).ok());
    if let Some(sid) = filter_session {
        if location
            .session_service
            .ensure_session_in_location(sid)
            .await
            .is_err()
        {
            let _ = filter_session.take();
            return error_sse(
                serde_json::json!({ "error": "session not found in this project", "code": "not_found" }),
            )
            .into_response();
        }
    }
    let store = location.session_service.event_store();
    let since = query.since_seq.unwrap_or(0).max(0);

    let live_rx = store.subscribe();
    let handoff_seq = store.latest_head_seq().await.unwrap_or(0);

    let history = store
        .tail_filtered(since, sse_replay_limit(), filter_session)
        .await
        .unwrap_or_default();

    let replay_limit = sse_replay_limit();
    // gap_too_large must still hand off to live: emit the error marker THEN
    // continue the live stream so the client recovers without reconnecting.
    let stream: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if history.len() as i64 >= replay_limit {
            let gap = tokio_stream::once(Ok(Event::default()
                .json_data(serde_json::json!({
                    "error": "gap_too_large",
                    "head": handoff_seq,
                    "limit": replay_limit,
                }))
                .unwrap_or_else(|_| Event::default())));
            let live = BroadcastStream::new(live_rx).filter_map(move |result| match result {
                Ok(e) => {
                    if filter_session
                        .map(|sid| e.session_id == Some(sid))
                        .unwrap_or(true)
                    {
                        Some(Ok(Event::default()
                            .json_data(e)
                            .unwrap_or_else(|_| Event::default())))
                    } else {
                        None
                    }
                }
                Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                    // Report the CURRENT head, not the stale handoff, so a
                    // reconnect does not loop on the same gap.
                    let skipped_i64 = i64::try_from(skipped).unwrap_or(i64::MAX);
                    Some(Ok(Event::default()
                        .json_data(serde_json::json!({
                            "error": "gap",
                            "head": handoff_seq.max(skipped_i64),
                            "limit": replay_limit,
                        }))
                        .unwrap_or_else(|_| Event::default())))
                }
            });
            Box::pin(gap.chain(live))
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
            let live = BroadcastStream::new(live_rx).filter_map(move |result| match result {
                Ok(e) => {
                    if e.seq > live_handoff
                        && filter_session
                            .map(|sid| e.session_id == Some(sid))
                            .unwrap_or(true)
                    {
                        Some(Ok(Event::default()
                            .json_data(e)
                            .unwrap_or_else(|_| Event::default())))
                    } else {
                        None
                    }
                }
                Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                    let skipped_i64 = i64::try_from(skipped).unwrap_or(i64::MAX);
                    Some(Ok(Event::default()
                        .json_data(serde_json::json!({
                            "error": "gap",
                            "head": live_handoff.max(skipped_i64),
                            "limit": replay_limit,
                        }))
                        .unwrap_or_else(|_| Event::default())))
                }
            });
            Box::pin(history_stream.chain(live))
        };

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
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
    let project_root = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
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
    let project_root = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    // Bound file size: refuse >8MiB over fs_file (use tool_result_page paging).
    if query.path.len() > 1024 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "path too long", "code": "invalid_input" })),
        )
            .into_response();
    }
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

    match std::fs::metadata(&path) {
        Ok(m) if m.len() > 8 * 1024 * 1024 => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(serde_json::json!({ "error": "file too large; use paging", "code": "too_large" })),
            )
                .into_response();
        }
        _ => {}
    }
    // Re-resolve after metadata check to narrow symlink-swap window; read
    // via the already-resolved absolute path.
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let truncated = if content.len() > 512 * 1024 {
                format!(
                    "{}…[truncated {} bytes]",
                    &content[..512 * 1024],
                    content.len() - 512 * 1024
                )
            } else {
                content
            };
            Json(serde_json::json!({ "content": truncated })).into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found", "code": "not_found" })),
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
    let project_root = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
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
        // Validate snapshot id shape to avoid path traversal via store lookup.
        if snapshot_id.len() > 128 || snapshot_id.contains(['/', '\\', '.', '\0']) {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "error": "invalid snapshot id", "code": "invalid_input" }),
                ),
            )
                .into_response();
        }
        let engine = state.engine.read().await;
        let store = deepseekcode_core::FileSnapshotStore::new(
            engine.db.pool().clone(),
            &project_root,
            true,
        );
        match store.get(snapshot_id).await {
            Ok(Some(snap)) => {
                // Snapshot files must live under the project snapshots dir;
                // re-resolve to block directory-escape via tampered DB rows.
                match resolve_within_project(&project_root, &snap.snapshot_path) {
                    Ok(p) => std::fs::read_to_string(&p).unwrap_or_default(),
                    Err(_) => {
                        // Fall back to canonical check: snapshot dir itself.
                        let canon = canonical_lossy(StdPath::new(&snap.snapshot_path));
                        let root = canonical_lossy(StdPath::new(&project_root));
                        if canon.starts_with(&root) {
                            std::fs::read_to_string(&canon).unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                }
            }
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
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    // Clamp rows/cols to sane bounds.
    let rows = req.rows.unwrap_or(24).clamp(5, 100);
    let cols = req.cols.unwrap_or(80).clamp(20, 300);
    match state.pty.create(&dir, rows, cols) {
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
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    if state.pty.check_ownership(&id, &dir).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "pty not found in this project", "code": "not_found" })),
        )
            .into_response();
    }
    match state
        .pty
        .resize(&id, req.rows.clamp(5, 100), req.cols.clamp(20, 300))
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/pty/{id}",
    tag = "pty",
    params(("id" = String, Path, description = "PTY session id")),
    responses((status = 200, description = "PTY destroyed"))
)]
pub(crate) async fn destroy_pty(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    match state.pty.destroy(&id, &dir) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => service_error_response(e).into_response(),
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
    // Auth is already enforced by basic_auth_middleware. The old magic
    // `x-deepseekcode-ticket:1` header was security theater (anyone could
    // send it) and is removed: an authenticated caller may issue a ticket.
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    match state.pty.issue_ticket(&id, &dir) {
        Ok((ticket, expires_in)) => Json(PtyTicketResponse { ticket, expires_in }).into_response(),
        Err(e) => service_error_response(e).into_response(),
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
    let dir = match validated_project_root(&headers) {
        Ok(d) => d,
        Err(resp) => return resp.into_response(),
    };
    // Always require a one-time ticket when a password is set; without a
    // password (local loopback) still verify project ownership.
    if state.password.is_some() {
        let ticket = query.ticket.as_deref().unwrap_or_default();
        if ticket.is_empty() || !state.pty.consume_ticket(ticket, &id, &dir) {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "invalid or expired PTY ticket", "code": "forbidden" })),
            )
                .into_response();
        }
    } else if state.pty.check_ownership(&id, &dir).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "pty not found in this project", "code": "not_found" })),
        )
            .into_response();
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
    // Use location-scoped lookup so cross-project IDs return 404, not data.
    match location
        .session_service
        .ensure_session_in_location(session_id)
        .await
    {
        Ok(session) => Json(session).into_response(),
        Err(e) => service_error_response(e).into_response(),
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
    // P1-10: unknown agent names are rejected (400) instead of silently
    // becoming Build; callers (CLI/desktop) only send plan/general/build.
    let agent = match req.agent.as_deref() {
        None | Some("build") => deepseekcode_schema::agent::AgentKind::Build,
        Some("plan") => deepseekcode_schema::agent::AgentKind::Plan,
        Some("general") => deepseekcode_schema::agent::AgentKind::General,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("unknown agent: {other}"),
                    "code": "invalid_input",
                })),
            )
                .into_response();
        }
    };
    let location = {
        let engine = state.engine.read().await;
        engine.location(&dir).await
    };
    match location
        .session_service
        .update_model(session_id, agent, req.model_id, req.provider_id)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
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
            service_error_response(e).into_response()
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
pub(crate) async fn export_diagnostics(
    State(_state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DiagnosticsQuery>,
) -> impl IntoResponse {
    // Directory-scoped: rejecting forbidden dirs prevents any authed user
    // from pulling diagnostics for another project context.
    if validated_project_root(&headers).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "directory not allowed", "code": "forbidden" })),
        )
            .into_response();
    }
    // Validate run_id to block path traversal (`../`, separators, NUL).
    if let Some(run_id) = query.run_id.as_deref() {
        if run_id.len() > 128
            || run_id.contains(['/', '\\', '\0'])
            || run_id.contains("..")
            || !run_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid run_id", "code": "invalid_input" })),
            )
                .into_response();
        }
    }
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
