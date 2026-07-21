use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_server::{build_router, pty::PtyManager, state::AppState};
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

struct TestServer {
    base_url: String,
    engine: Arc<RwLock<DeepSeekCodeEngine>>,
    _db: TempDir,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn abort(self) {
        self.handle.abort();
    }
}

async fn test_app(project_dir: &str) -> TestServer {
    let db_dir = TempDir::new().expect("db tempdir");
    let db_path = format!("sqlite://{}/deepseekcode-test.db", db_dir.path().display());
    let engine = Arc::new(RwLock::new(
        DeepSeekCodeEngine::open(project_dir, &db_path)
            .await
            .expect("open engine"),
    ));
    let state = AppState {
        engine: engine.clone(),
        password: None,
        pty: Arc::new(PtyManager::new()),
    };
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test app");
    });
    TestServer {
        base_url: format!("http://{addr}"),
        engine,
        _db: db_dir,
        handle,
    }
}

fn dir_header(project_path: &str) -> reqwest::header::HeaderValue {
    reqwest::header::HeaderValue::from_str(project_path).expect("header value")
}

fn normalized_project_path(dir: &std::path::Path) -> String {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
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

#[tokio::test]
async fn health_and_openapi_endpoints() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");

    let server = test_app(&normalized_project_path(dir.path())).await;
    let client = reqwest::Client::new();

    let health: serde_json::Value = client
        .get(format!("{}/api/v1/health", server.base_url))
        .send()
        .await
        .expect("health request")
        .json()
        .await
        .expect("health json");
    assert_eq!(health["status"], "ok");

    let alias: serde_json::Value = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .expect("health alias request")
        .json()
        .await
        .expect("health alias json");
    assert_eq!(alias["status"], "ok");

    let openapi: serde_json::Value = client
        .get(format!("{}/api/v1/openapi.json", server.base_url))
        .send()
        .await
        .expect("openapi request")
        .json()
        .await
        .expect("openapi json");
    assert_eq!(openapi["info"]["title"], "DeepSeekCode API");
    let paths = openapi["paths"].as_object().expect("paths object");
    assert!(paths.contains_key("/api/v1/session/active"));
    assert!(paths.contains_key("/api/v1/session/{id}/fork"));
    assert!(paths.contains_key("/api/v1/mcp"));
    assert!(paths.contains_key("/api/v1/openapi.json"));

    let expected = [
        "/health",
        "/api/v1/health",
        "/api/v1/openapi.json",
        "/api/v1/location",
        "/api/v1/projects",
        "/api/v1/project/directories",
        "/api/v1/worktree",
        "/api/v1/worktree/prune",
        "/api/v1/session",
        "/api/v1/session/active",
        "/api/v1/session/{id}/prompt",
        "/api/v1/session/{id}/interrupt",
        "/api/v1/session/{id}/messages",
        "/api/v1/session/{id}/timeline",
        "/api/v1/session/{id}/activity",
        "/api/v1/session/{id}/todo",
        "/api/v1/session/{id}/compact",
        "/api/v1/session/{id}/fork",
        "/api/v1/session/{id}/revert",
        "/api/v1/session/{id}/revert/stage",
        "/api/v1/session/{id}/revert/clear",
        "/api/v1/session/{id}/revert/commit",
        "/api/v1/session/{id}/move",
        "/api/v1/session/{id}/export",
        "/api/v1/session/{id}/model",
        "/api/v1/provider",
        "/api/v1/provider/{id}/connect",
        "/api/v1/provider/{id}/disconnect",
        "/api/v1/permission/pending",
        "/api/v1/permission/{id}/respond",
        "/api/v1/policy/pending",
        "/api/v1/policy/{id}/reply",
        "/api/v1/policy/grants",
        "/api/v1/policy/grants/{id}",
        "/api/v1/policy/config",
        "/api/v1/mcp",
        "/api/v1/mcp/{name}/connect",
        "/api/v1/mcp/{name}/disconnect",
        "/api/v1/skills",
        "/api/v1/plugins",
        "/api/v1/diagnostics/export",
        "/api/v1/event",
        "/api/v1/fs/tree",
        "/api/v1/fs/file",
        "/api/v1/fs/diff",
        "/api/v1/pty",
        "/api/v1/pty/{id}/ticket",
        "/api/v1/pty/{id}/connect",
    ];
    for path in expected {
        assert!(paths.contains_key(path), "openapi missing path: {path}");
    }

    server.abort();
}

#[tokio::test]
async fn projects_crud_roundtrip() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/projects", server.base_url))
        .json(&serde_json::json!({ "path": project_path }))
        .send()
        .await
        .expect("create project")
        .json()
        .await
        .expect("create project json");
    assert_eq!(created["path"], project_path);

    let listed: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/projects", server.base_url))
        .send()
        .await
        .expect("list projects")
        .json()
        .await
        .expect("list projects json");
    assert!(listed.iter().any(|p| p["path"] == project_path));

    server.abort();
}

#[tokio::test]
async fn fs_tree_rejects_paths_outside_project_root() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{}/api/v1/fs/file", server.base_url))
        .header("x-deepseekcode-directory", dir_header(&project_path))
        .query(&[("path", "C:\\Windows\\System32\\drivers\\etc\\hosts")])
        .send()
        .await
        .expect("fs file request");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    server.abort();
}

#[tokio::test]
async fn session_prompt_mock_llm_flow() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nTest project.").expect("write agents");
    let project_path = normalized_project_path(dir.path());
    let moved_path = dir.path().join("moved");
    std::fs::create_dir_all(&moved_path).expect("moved dir");
    let moved_path = normalized_project_path(&moved_path);

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    let prompt_resp: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/prompt",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "session_id": session_id,
            "prompt": "hello without llm",
            "delivery": "queue",
            "resume": false
        }))
        .send()
        .await
        .expect("prompt")
        .json()
        .await
        .expect("prompt json");
    assert!(prompt_resp["message_id"].is_string());

    let messages: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/messages",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("list messages")
        .json()
        .await
        .expect("messages json");
    assert!(messages.is_empty());

    let active: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/session/active", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("active sessions")
        .json()
        .await
        .expect("active json");
    assert!(
        active.iter().all(|id| id.as_str() != Some(session_id)),
        "prompt with resume=false should not mark session active"
    );

    let forked: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/fork",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("fork")
        .json()
        .await
        .expect("fork json");
    let fork_id = forked["id"].as_str().expect("fork id");
    assert_ne!(fork_id, session_id);

    let export_resp = client
        .get(format!(
            "{}/api/v1/session/{session_id}/export",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("export");
    assert!(export_resp.status().is_success(), "export failed");
    let export: serde_json::Value = export_resp.json().await.expect("export json");
    assert_eq!(export["version"], 2);
    assert_eq!(export["session"]["id"], session_id);
    assert!(export["messages"].is_array());
    assert!(export["timeline"].is_array());

    let clear: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/revert/clear",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("revert clear")
        .json()
        .await
        .expect("revert clear json");
    assert_eq!(clear["ok"], true);

    let move_resp: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/move",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({ "directory": moved_path }))
        .send()
        .await
        .expect("move session")
        .json()
        .await
        .expect("move json");
    assert_eq!(move_resp["ok"], true);

    let skills: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/skills", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("skills")
        .json()
        .await
        .expect("skills json");
    assert!(skills.is_empty() || skills[0]["name"].is_string());

    let mcp: serde_json::Value = client
        .get(format!("{}/api/v1/mcp", server.base_url))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("mcp")
        .json()
        .await
        .expect("mcp json");
    assert!(mcp.is_object() || mcp.is_array());

    server.abort();
}

#[tokio::test]
async fn compact_session_endpoint() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    let compact: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/compact",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("compact")
        .json()
        .await
        .expect("compact json");
    assert_eq!(compact["ok"], true);

    server.abort();
}

#[tokio::test]
async fn permission_respond_roundtrip() {
    use deepseekcode_schema::agent::AgentKind;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    std::fs::write(
        dir.path().join("deepseekcode.jsonc"),
        r#"{"policy":{"bash":{"*":"prompt"}}}"#,
    )
    .expect("write config");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_id = deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    let permissions = {
        let engine = server.engine.read().await;
        let location = engine.location(&project_path).await;
        location.permissions.clone()
    };
    let wait = tokio::spawn(async move {
        let _ = permissions
            .ensure_allowed(
                session_id,
                "bash",
                &AgentKind::Build,
                serde_json::json!({ "command": "echo hi" }),
            )
            .await;
    });
    let mut pending: Vec<serde_json::Value> = Vec::new();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        pending = client
            .get(format!("{}/api/v1/policy/pending", server.base_url))
            .header("x-deepseekcode-directory", hdr.clone())
            .send()
            .await
            .expect("pending")
            .json()
            .await
            .expect("pending json");
        if pending.len() == 1 {
            break;
        }
    }

    assert_eq!(pending.len(), 1, "expected one pending permission");
    let permission_id = pending[0]["id"].as_str().expect("permission id");

    let respond: serde_json::Value = client
        .post(format!(
            "{}/api/v1/policy/{permission_id}/reply",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr)
        .json(&serde_json::json!({ "reply": "reject" }))
        .send()
        .await
        .expect("respond")
        .json()
        .await
        .expect("respond json");
    assert_eq!(respond["ok"], true);
    let _ = wait.await;

    server.abort();
}

#[tokio::test]
async fn policy_project_grant_persists() {
    use deepseekcode_schema::agent::AgentKind;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    std::fs::write(
        dir.path().join("deepseekcode.jsonc"),
        r#"{"policy":{"bash":{"*":"prompt"}}}"#,
    )
    .expect("write config");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_id = deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    let permissions = {
        let engine = server.engine.read().await;
        let location = engine.location(&project_path).await;
        location.permissions.clone()
    };
    let wait = tokio::spawn(async move {
        let _ = permissions
            .ensure_allowed(
                session_id,
                "bash",
                &AgentKind::Build,
                serde_json::json!({ "command": "echo grant" }),
            )
            .await;
    });
    let mut pending: Vec<serde_json::Value> = Vec::new();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        pending = client
            .get(format!("{}/api/v1/policy/pending", server.base_url))
            .header("x-deepseekcode-directory", hdr.clone())
            .send()
            .await
            .expect("pending")
            .json()
            .await
            .expect("pending json");
        if pending.len() == 1 {
            break;
        }
    }
    assert_eq!(pending.len(), 1, "expected one pending policy");
    let policy_id = pending[0]["id"].as_str().expect("policy id");

    let respond: serde_json::Value = client
        .post(format!(
            "{}/api/v1/policy/{policy_id}/reply",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({ "reply": "project" }))
        .send()
        .await
        .expect("respond")
        .json()
        .await
        .expect("respond json");
    assert_eq!(respond["ok"], true);
    let _ = wait.await;

    let grants: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/policy/grants", server.base_url))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("grants")
        .json()
        .await
        .expect("grants json");
    assert!(!grants.is_empty(), "expected at least one project grant");

    server.abort();
}

#[tokio::test]
async fn update_policy_config_roundtrip() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    std::fs::write(dir.path().join("deepseekcode.jsonc"), r#"{}"#).expect("write config");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let resp: serde_json::Value = client
        .put(format!("{}/api/v1/policy/config", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "policy": {
                "bash": { "*": "prompt" }
            }
        }))
        .send()
        .await
        .expect("update policy")
        .json()
        .await
        .expect("update policy json");
    assert_eq!(resp["ok"], true);

    let cfg = deepseekcode_config::load_effective_config(camino::Utf8Path::new(&project_path))
        .expect("reload config");
    assert!(cfg.policy.contains_key("bash"));

    server.abort();
}

#[tokio::test]
async fn legacy_permission_respond_roundtrip() {
    use deepseekcode_schema::agent::AgentKind;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    std::fs::write(
        dir.path().join("deepseekcode.jsonc"),
        r#"{"policy":{"bash":{"*":"prompt"}}}"#,
    )
    .expect("write config");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_id = deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    let permissions = {
        let engine = server.engine.read().await;
        let location = engine.location(&project_path).await;
        location.permissions.clone()
    };
    let wait = tokio::spawn(async move {
        let _ = permissions
            .ensure_allowed(
                session_id,
                "bash",
                &AgentKind::Build,
                serde_json::json!({ "command": "echo legacy" }),
            )
            .await;
    });
    let mut pending: Vec<serde_json::Value> = Vec::new();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        pending = client
            .get(format!("{}/api/v1/permission/pending", server.base_url))
            .header("x-deepseekcode-directory", hdr.clone())
            .send()
            .await
            .expect("pending")
            .json()
            .await
            .expect("pending json");
        if pending.len() == 1 {
            break;
        }
    }
    assert_eq!(pending.len(), 1, "expected one pending permission");
    let permission_id = pending[0]["id"].as_str().expect("permission id");

    let respond: serde_json::Value = client
        .post(format!(
            "{}/api/v1/permission/{permission_id}/respond",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr)
        .json(&serde_json::json!({ "status": "denied" }))
        .send()
        .await
        .expect("respond")
        .json()
        .await
        .expect("respond json");
    assert_eq!(respond["ok"], true);
    let _ = wait.await;

    server.abort();
}

#[tokio::test]
async fn policy_auto_mode_skips_prompt() {
    use deepseekcode_schema::agent::AgentKind;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    std::fs::write(
        dir.path().join("deepseekcode.jsonc"),
        r#"{"policy_mode":"auto","policy":{"bash":{"*":"prompt"}}}"#,
    )
    .expect("write config");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_id = deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    let permissions = {
        let engine = server.engine.read().await;
        let location = engine.location(&project_path).await;
        location.permissions.clone()
    };
    let result = permissions
        .ensure_allowed(
            session_id,
            "bash",
            &AgentKind::Build,
            serde_json::json!({ "command": "echo hi" }),
        )
        .await;
    assert!(result.is_ok(), "auto mode should permit prompt rules");

    let pending: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/policy/pending", server.base_url))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("pending")
        .json()
        .await
        .expect("pending json");
    assert!(
        pending.is_empty(),
        "auto mode should not leave pending prompts"
    );

    server.abort();
}

#[tokio::test]
async fn sse_event_stream_delivers_prompt_admitted() {
    use futures::StreamExt;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    let prompt_resp: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/prompt",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "session_id": session_id,
            "prompt": "event test",
            "delivery": "queue",
            "resume": false
        }))
        .send()
        .await
        .expect("prompt")
        .json()
        .await
        .expect("prompt json");
    assert!(prompt_resp["message_id"].is_string());

    let response = client
        .get(format!(
            "{}/api/v1/event?since_seq=0&session_id={session_id}",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("event stream");

    let mut collected = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        collected.extend_from_slice(&chunk);
        if collected.len() > 256 {
            break;
        }
    }
    let body = String::from_utf8_lossy(&collected);
    assert!(
        body.contains("prompt_admitted")
            || body.contains("prompt.admitted")
            || body.contains("PromptAdmitted"),
        "expected prompt admitted event in SSE body, got: {body}"
    );

    server.abort();
}

#[tokio::test]
async fn prompt_resume_mock_llm_verifies_completion_events() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock = MockServer::start().await;
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&mock)
        .await;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nTest project.").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);
    let mock_base = format!("{}/v1", mock.uri());

    client
        .post(format!(
            "{}/api/v1/provider/openai/connect",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "api_key": "test-key",
            "base_url": mock_base
        }))
        .send()
        .await
        .expect("connect provider");

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build",
            "provider_id": "openai",
            "model_id": "gpt-4o-mini"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    client
        .post(format!(
            "{}/api/v1/session/{session_id}/prompt",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "session_id": session_id,
            "prompt": "ping",
            "delivery": "queue",
            "resume": true
        }))
        .send()
        .await
        .expect("prompt");

    let mut saw_assistant = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let messages: Vec<serde_json::Value> = client
            .get(format!(
                "{}/api/v1/session/{session_id}/messages",
                server.base_url
            ))
            .header("x-deepseekcode-directory", hdr.clone())
            .send()
            .await
            .expect("messages")
            .json()
            .await
            .expect("messages json");
        if messages.iter().any(|m| m["role"] == "assistant") {
            saw_assistant = true;
            break;
        }
    }
    assert!(saw_assistant, "expected assistant message from mock LLM");

    let session_uuid =
        deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");
    let mut saw_completion = false;
    for _ in 0..40 {
        let events = {
            let engine = server.engine.read().await;
            let location = engine.location(&project_path).await;
            location
                .session_service
                .event_store()
                .tail_filtered(0, 100, Some(session_uuid))
                .await
                .expect("tail events")
        };
        if events.iter().any(|e| {
            matches!(
                e.kind,
                deepseekcode_schema::event::EventKind::MessageCompleted
                    | deepseekcode_schema::event::EventKind::SessionIdle
            )
        }) {
            saw_completion = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        saw_completion,
        "expected message.completed or session.idle event"
    );

    server.abort();
}

#[tokio::test]
async fn prompt_resume_mock_llm_tool_call_pipeline() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock = MockServer::start().await;
    let tool_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_read\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\"\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"README.md\\\"}\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );
    let text_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(tool_sse)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(text_sse)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&mock)
        .await;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nTest project.").expect("write agents");
    std::fs::write(dir.path().join("README.md"), "# readme").expect("write readme");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);
    let mock_base = format!("{}/v1", mock.uri());

    client
        .post(format!(
            "{}/api/v1/provider/openai/connect",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "api_key": "test-key",
            "base_url": mock_base
        }))
        .send()
        .await
        .expect("connect provider");

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build",
            "provider_id": "openai",
            "model_id": "gpt-4o-mini"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    client
        .post(format!(
            "{}/api/v1/session/{session_id}/prompt",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "session_id": session_id,
            "prompt": "read README",
            "delivery": "queue",
            "resume": true
        }))
        .send()
        .await
        .expect("prompt");

    let session_uuid =
        deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");
    let mut saw_tool_started = false;
    let mut saw_tool_completed = false;
    let mut saw_idle = false;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let events = {
            let engine = server.engine.read().await;
            let location = engine.location(&project_path).await;
            location
                .session_service
                .event_store()
                .tail_filtered(0, 200, Some(session_uuid))
                .await
                .expect("tail events")
        };
        saw_tool_started |= events
            .iter()
            .any(|e| matches!(e.kind, deepseekcode_schema::event::EventKind::ToolStarted));
        saw_tool_completed |= events
            .iter()
            .any(|e| matches!(e.kind, deepseekcode_schema::event::EventKind::ToolCompleted));
        saw_idle |= events
            .iter()
            .any(|e| matches!(e.kind, deepseekcode_schema::event::EventKind::SessionIdle));
        if saw_tool_started && saw_tool_completed && saw_idle {
            break;
        }
    }

    assert!(saw_tool_started, "expected tool.started in mock tool E2E");
    assert!(
        saw_tool_completed,
        "expected tool.completed in mock tool E2E"
    );
    assert!(saw_idle, "expected session.idle after tool settlement");

    server.abort();
}

#[tokio::test]
async fn revert_stage_commit_restores_files_via_http() {
    use deepseekcode_storage::SessionStore;
    use deepseekcode_types::MessageId;

    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent").expect("agents");
    let demo = dir.path().join("demo.txt");
    std::fs::write(&demo, "original").expect("write demo");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_uuid =
        deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    let engine = server.engine.read().await;
    let location = engine.location(&project_path).await;
    let pool = engine.db.pool().clone();
    let store = SessionStore::new(pool.clone());
    let user_id = MessageId::new();
    store
        .insert_message(user_id, session_uuid, "user", "edit file")
        .await
        .expect("insert user message");

    let snapshots =
        deepseekcode_core::snapshot::FileSnapshotStore::new(pool.clone(), dir.path(), true);
    let tool_msg = MessageId::new();
    snapshots
        .capture(session_uuid, Some(tool_msg), &demo)
        .await
        .expect("capture snapshot");
    std::fs::write(&demo, "mutated").expect("mutate demo");
    store
        .insert_tool_result(
            tool_msg,
            session_uuid,
            0,
            "call-1",
            "write",
            &serde_json::json!({ "path": "demo.txt" }),
        )
        .await
        .expect("insert tool result");
    drop(location);
    drop(engine);

    let staged: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/revert/stage",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({ "message_id": user_id.to_string() }))
        .send()
        .await
        .expect("revert stage")
        .json()
        .await
        .expect("revert stage json");
    assert_eq!(
        staged["staged_message_id"].as_str(),
        Some(user_id.to_string().as_str())
    );
    assert_eq!(
        std::fs::read_to_string(&demo).expect("read demo"),
        "original"
    );

    let committed: serde_json::Value = client
        .post(format!(
            "{}/api/v1/session/{session_id}/revert/commit",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("revert commit")
        .json()
        .await
        .expect("revert commit json");
    assert!(committed["staged_message_id"].is_string());

    let messages: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/messages",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr)
        .send()
        .await
        .expect("list messages")
        .json()
        .await
        .expect("messages json");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str(), Some("user"));

    server.abort();
}

#[tokio::test]
async fn session_activity_and_todo_endpoints() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());

    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "build"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");

    let activity: serde_json::Value = client
        .get(format!(
            "{}/api/v1/session/{session_id}/activity",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("activity")
        .json()
        .await
        .expect("activity json");
    assert!(activity["roots"].is_array());

    let empty_todos: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/todo",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("get todo")
        .json()
        .await
        .expect("todo json");
    assert!(empty_todos.is_empty());

    let updated: Vec<serde_json::Value> = client
        .post(format!(
            "{}/api/v1/session/{session_id}/todo",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "todos": [{
                "content": "Ship feature",
                "status": "pending",
                "priority": "high"
            }]
        }))
        .send()
        .await
        .expect("post todo")
        .json()
        .await
        .expect("post todo json");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["content"], "Ship feature");

    let listed: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/todo",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("get todo again")
        .json()
        .await
        .expect("todo json again");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["status"], "pending");

    let activity_after: serde_json::Value = client
        .get(format!(
            "{}/api/v1/session/{session_id}/activity",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("activity after todo")
        .json()
        .await
        .expect("activity after todo json");
    let activity_str = activity_after.to_string();
    assert!(
        activity_str.contains("todo_write"),
        "expected todo_write leaf in activity tree after todo update, got {activity_str}"
    );

    server.abort();
}

#[tokio::test]
async fn session_plans_list_and_dismiss() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# test").expect("write agents");
    let project_path = normalized_project_path(dir.path());
    let server = test_app(&project_path).await;
    let client = reqwest::Client::new();
    let hdr = dir_header(&project_path);

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .json(&serde_json::json!({
            "directory": project_path,
            "agent": "plan"
        }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    let session_id = session["id"].as_str().expect("session id");
    let session_uuid =
        deepseekcode_types::SessionId::from_str(session_id).expect("parse session id");

    {
        let engine = server.engine.read().await;
        let location = engine.location(&project_path).await;
        location
            .session_service
            .event_store()
            .append(
                deepseekcode_schema::event::EventKind::PlanCompleted,
                Some(session_uuid),
                serde_json::json!({
                    "plan_id": "auth-refactor-20260716-a3f2",
                    "plan_path": ".deepseekcode/plan/auth-refactor-20260716-a3f2/PLAN.md",
                    "title": "Auth refactor",
                    "summary": "Step-by-step plan",
                    "session_id": session_id,
                }),
            )
            .await
            .expect("append plan.completed");
    }

    let plans: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/plans",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("list plans")
        .json()
        .await
        .expect("plans json");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0]["plan_id"], "auth-refactor-20260716-a3f2");
    assert!(!plans[0]["implement_prompt_dismissed"].as_bool().unwrap());

    let dismiss = client
        .post(format!(
            "{}/api/v1/session/{session_id}/plans/auth-refactor-20260716-a3f2/dismiss",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("dismiss plan");
    assert!(dismiss.status().is_success());

    let plans_after: Vec<serde_json::Value> = client
        .get(format!(
            "{}/api/v1/session/{session_id}/plans",
            server.base_url
        ))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("list plans again")
        .json()
        .await
        .expect("plans json again");
    assert!(plans_after[0]["implement_prompt_dismissed"]
        .as_bool()
        .unwrap());

    let fetched: serde_json::Value = client
        .get(format!("{}/api/v1/session/{session_id}", server.base_url))
        .header("x-deepseekcode-directory", hdr.clone())
        .send()
        .await
        .expect("get session")
        .json()
        .await
        .expect("session json");
    assert_eq!(fetched["agent"], "plan");

    server.abort();
}

#[tokio::test]
async fn session_cross_directory_access_returns_not_found() {
    let dir_a = TempDir::new().expect("tempdir a");
    let dir_b = TempDir::new().expect("tempdir b");
    std::fs::write(dir_a.path().join("AGENTS.md"), "# a").expect("agents a");
    std::fs::write(dir_b.path().join("AGENTS.md"), "# b").expect("agents b");
    let path_a = normalized_project_path(dir_a.path());
    let path_b = normalized_project_path(dir_b.path());

    let server = test_app(&path_a).await;
    let client = reqwest::Client::new();

    let session: serde_json::Value = client
        .post(format!("{}/api/v1/session", server.base_url))
        .header("x-deepseekcode-directory", &path_a)
        .json(&serde_json::json!({ "directory": path_a }))
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    let resp = client
        .get(format!("{}/api/v1/session/{session_id}", server.base_url))
        .header("x-deepseekcode-directory", &path_b)
        .send()
        .await
        .expect("cross get");
    assert_eq!(resp.status(), 404);

    let resp = client
        .post(format!(
            "{}/api/v1/session/{session_id}/prompt",
            server.base_url
        ))
        .header("x-deepseekcode-directory", &path_b)
        .json(&serde_json::json!({
            "session_id": session_id,
            "prompt": "hello",
            "delivery": "queue",
            "resume": false
        }))
        .send()
        .await
        .expect("cross prompt");
    assert_eq!(resp.status(), 404);

    server.abort();
}
