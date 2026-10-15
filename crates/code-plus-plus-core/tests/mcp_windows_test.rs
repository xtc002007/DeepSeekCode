//! windows-mcp style stdio server → connect/list/call coverage.
//!
//! Reference: `my-notebook` stdio MCP transport (`STDIO_TIMEOUT`, exact id
//! matching) + this repo's `mcp_session_test.rs` echo-server pattern.

use code_plus_plus_config::schema::McpServerConfig;
use code_plus_plus_core::mcp::McpConnectionPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn windows_server_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/mcp_windows_server.py")
}

fn python_command() -> String {
    if cfg!(windows) {
        "python".into()
    } else {
        "python3".into()
    }
}

fn tool_text(result: &serde_json::Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn windows_mcp_stdio_connect_lists_desktop_tools() {
    let script = windows_server_script();
    let python = python_command();
    if std::process::Command::new(&python)
        .arg(&script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_err()
    {
        eprintln!("skip: {python} unavailable for windows-mcp server");
        return;
    }

    let mut servers = HashMap::new();
    servers.insert(
        "windows-desktop".into(),
        McpServerConfig {
            command: Some(python),
            args: Some(vec![script.to_string_lossy().to_string()]),
            url: None,
            enabled: Some(true),
        },
    );

    let pool = Arc::new(McpConnectionPool::new(servers));
    let tools = pool
        .connect("windows-desktop")
        .await
        .expect("windows-mcp connect");
    for expected in ["screenshot", "snapshot", "click", "type_text", "powershell"] {
        assert!(
            tools.iter().any(|t| t == expected),
            "expected desktop tool {expected} in {tools:?}"
        );
    }

    let shot = pool
        .call_tool("windows-desktop", "screenshot", serde_json::json!({}))
        .await
        .expect("screenshot call");
    assert!(tool_text(&shot).contains("mocked-screenshot-ok"));

    let clicked = pool
        .call_tool(
            "windows-desktop",
            "click",
            serde_json::json!({"x": 100, "y": 200}),
        )
        .await
        .expect("click call");
    assert_eq!(tool_text(&clicked), "clicked x=100 y=200");

    let typed = pool
        .call_tool(
            "windows-desktop",
            "type_text",
            serde_json::json!({"text": "hello"}),
        )
        .await
        .expect("type call");
    assert_eq!(tool_text(&typed), "typed 5 chars");

    // Unknown tool surfaces the server-side JSON-RPC error.
    let err = pool
        .call_tool("windows-desktop", "nope", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown tool"));
}
