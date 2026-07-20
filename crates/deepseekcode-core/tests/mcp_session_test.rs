//! MCP stdio subprocess → bridged tool → mock LLM session settlement.

use deepseekcode_config::schema::McpServerConfig;
use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_core::mcp::McpConnectionPool;
use deepseekcode_core::session::SessionService;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest};
use deepseekcode_storage::{Database, EventStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn mcp_echo_server_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/mcp_echo_server.py")
}

fn python_command() -> String {
    if cfg!(windows) {
        "python".into()
    } else {
        "python3".into()
    }
}

fn skip_if_no_python(script: &PathBuf) -> Option<String> {
    let python = python_command();
    let ok = std::process::Command::new(&python)
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if ok {
        None
    } else {
        Some(format!("{python} unavailable for MCP echo server"))
    }
}

#[tokio::test]
async fn mcp_stdio_connect_lists_and_calls_echo_tool() {
    let script = mcp_echo_server_script();
    if let Some(reason) = skip_if_no_python(&script) {
        eprintln!("skip mcp_stdio_connect_lists_and_calls_echo_tool: {reason}");
        return;
    }

    let mut servers = HashMap::new();
    servers.insert(
        "echo".into(),
        McpServerConfig {
            command: Some(python_command()),
            args: Some(vec![script.to_string_lossy().to_string()]),
            url: None,
            enabled: Some(true),
        },
    );

    let pool = Arc::new(McpConnectionPool::new(servers));
    let tools = pool.connect("echo").await.expect("mcp connect");
    assert!(tools.iter().any(|t| t == "echo"));

    let result = pool
        .call_tool("echo", "echo", serde_json::json!({ "message": "hello-mcp" }))
        .await
        .expect("mcp call");
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    assert_eq!(text, "hello-mcp");
}

#[tokio::test]
async fn mcp_tool_invoked_in_mock_llm_session() {
    let script = mcp_echo_server_script();
    if let Some(reason) = skip_if_no_python(&script) {
        eprintln!("skip mcp_tool_invoked_in_mock_llm_session: {reason}");
        return;
    }

    let mock = MockServer::start().await;
    let tool_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_mcp\",\"function\":{\"name\":\"echo_echo\",\"arguments\":\"{\\\"message\\\"\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"from-mcp\\\"}\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );
    let text_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"mcp done\"},\"index\":0}]}\n\n",
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

    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nMCP test.").unwrap();

    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let events = Arc::new(EventStore::new(db.pool().clone()));

    let mut servers = HashMap::new();
    servers.insert(
        "echo".into(),
        McpServerConfig {
            command: Some(python_command()),
            args: Some(vec![script.to_string_lossy().to_string()]),
            url: None,
            enabled: Some(true),
        },
    );
    let mcp = Arc::new(McpConnectionPool::new(servers).with_events(events.clone()));
    mcp.connect("echo").await.expect("mcp connect");

    let providers = Arc::new(deepseekcode_core::provider::ProviderCatalog::new());
    providers
        .connect(
            "openai",
            "test-key".into(),
            Some(format!("{}/v1", mock.uri())),
        )
        .await
        .unwrap();

    let mut perms_config = deepseekcode_config::schema::DeepSeekCodeConfig::default();
    perms_config.permissions.allow.push("mcp:echo:*".into());
    let permissions = Arc::new(deepseekcode_core::permission::PermissionService::new(
        &perms_config,
        dir.path().to_string_lossy(),
        None,
    ));
    let hooks = Arc::new(deepseekcode_plugin::HookRegistry::new());
    let service = SessionService::new(
        db,
        DeepSeekCodeConfig::default(),
        dir.path().to_string_lossy().to_string(),
        permissions,
        providers,
        events.clone(),
        mcp,
        hooks,
        None,
    );

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: Some("gpt-4o-mini".into()),
            provider_id: Some("openai".into()),
        })
        .await
        .unwrap();

    service
        .prompt(PromptRequest {
            id: None,
            session_id: session.id,
            prompt: "use mcp echo".into(),
            delivery: PromptDelivery::Queue,
            resume: true,
        })
        .await
        .unwrap();

    let mut saw_mcp_tool = false;
    let mut saw_idle = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let tail = events
            .tail_filtered(0, 200, Some(session.id))
            .await
            .unwrap();
        saw_mcp_tool |= tail.iter().any(|e| {
            matches!(e.kind, EventKind::ToolCompleted)
                && e.payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == "echo_echo")
        });
        saw_idle |= tail
            .iter()
            .any(|e| matches!(e.kind, EventKind::SessionIdle));
        if saw_mcp_tool && saw_idle {
            break;
        }
    }

    assert!(saw_mcp_tool, "expected echo_echo tool.completed from MCP bridge");
    assert!(saw_idle, "expected session.idle after MCP tool settlement");
}
