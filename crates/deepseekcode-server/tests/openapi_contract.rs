//! Ensures live OpenAPI JSON stays aligned with utoipa registration.

use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_server::{build_router, pty::PtyManager, state::AppState};
use std::collections::BTreeSet;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn test_server() -> (String, tokio::task::JoinHandle<()>, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\n").ok();
    let db_dir = TempDir::new().expect("db tempdir");
    let db_path = format!("sqlite://{}/test-openapi.db", db_dir.path().display());
    let project_dir = dir.path().to_string_lossy().to_string();
    let engine = Arc::new(RwLock::new(
        DeepSeekCodeEngine::open(&project_dir, &db_path)
            .await
            .expect("engine"),
    ));
    let state = AppState {
        engine,
        password: None,
        pty: Arc::new(PtyManager::new()),
        locale: Default::default(),
    };
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}"), handle, dir)
}

#[tokio::test]
async fn live_openapi_paths_match_utoipa_spec() {
    let (base_url, handle, _dir) = test_server().await;
    let live: serde_json::Value = reqwest::Client::new()
        .get(format!("{base_url}/api/v1/openapi.json"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let spec = deepseekcode_server::openapi::spec();
    let live_paths: BTreeSet<String> = live["paths"]
        .as_object()
        .expect("paths")
        .keys()
        .cloned()
        .collect();
    let spec_paths: BTreeSet<String> = spec.paths.paths.keys().cloned().collect();
    assert_eq!(
        live_paths, spec_paths,
        "live openapi paths drift from utoipa spec"
    );
    handle.abort();
}

#[tokio::test]
async fn openapi_includes_policy_schemas() {
    let spec = deepseekcode_server::openapi::spec();
    let schemas = &spec.components.as_ref().expect("components").schemas;
    for name in ["PolicyRequest", "PolicyReply", "PolicyGrant"] {
        assert!(
            schemas.contains_key(name),
            "missing schema component: {name}"
        );
    }
}

#[tokio::test]
async fn invalid_session_id_returns_400() {
    let (base_url, handle, dir) = test_server().await;
    let path = dir.path().to_string_lossy().to_string();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base_url}/api/v1/session/not-a-valid-ulid"))
        .header("x-deepseekcode-directory", &path)
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 400);
    handle.abort();
}

#[tokio::test]
async fn sse_gap_too_large_when_replay_exceeds_limit() {
    std::env::set_var("DEEPSEEKCODE_SSE_REPLAY_LIMIT", "3");
    let (base_url, handle, dir) = test_server().await;
    let path = dir.path().to_string_lossy().to_string();
    let client = reqwest::Client::new();

    let session: serde_json::Value = client
        .post(format!("{base_url}/api/v1/session"))
        .header("x-deepseekcode-directory", &path)
        .json(&serde_json::json!({ "directory": path }))
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("json");
    let session_id = session["id"].as_str().expect("id");

    for i in 0..5 {
        let _ = client
            .post(format!("{base_url}/api/v1/session/{session_id}/prompt"))
            .header("x-deepseekcode-directory", &path)
            .json(&serde_json::json!({
                "session_id": session_id,
                "prompt": format!("event seed {i}"),
                "delivery": "queue",
                "resume": false
            }))
            .send()
            .await;
    }

    let resp = client
        .get(format!(
            "{base_url}/api/v1/event?since_seq=0&session_id={session_id}"
        ))
        .header("x-deepseekcode-directory", &path)
        .send()
        .await
        .expect("sse");
    assert_eq!(resp.status(), 200);
    // Read incrementally: SSE is an infinite stream (history + live +
    // keep-alive), so `resp.text()` would wait for EOF forever. The gap
    // marker is the first frame when replay exceeds the limit.
    let body = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.expect("sse chunk");
            buf.extend_from_slice(&chunk);
            if buf.windows(13).any(|w| w == b"gap_too_large") || buf.len() > 65_536 {
                break;
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    })
    .await
    .expect("gap marker within 15s");
    assert!(
        body.contains("gap_too_large"),
        "expected gap marker in SSE body, got: {body}"
    );

    std::env::remove_var("DEEPSEEKCODE_SSE_REPLAY_LIMIT");
    handle.abort();
}

#[tokio::test]
async fn fs_write_requires_session_header_when_enabled() {
    std::env::set_var("DEEPSEEKCODE_ALLOW_FS_WRITE", "1");
    let (base_url, handle, dir) = test_server().await;
    let path = dir.path().to_string_lossy().to_string();
    let client = reqwest::Client::new();

    let resp = client
        .put(format!("{base_url}/api/v1/fs/file"))
        .header("x-deepseekcode-directory", &path)
        .json(&serde_json::json!({ "path": "test.txt", "content": "hi" }))
        .send()
        .await
        .expect("fs write");
    assert_eq!(resp.status(), 400);

    std::env::remove_var("DEEPSEEKCODE_ALLOW_FS_WRITE");
    handle.abort();
}
