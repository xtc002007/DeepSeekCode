//! End-to-end regression for the interactive Shell tab.
//!
//! The desktop used to open the PTY WebSocket with a bare
//! `connect_async(url)` carrying no `x-code-plus-plus-directory` header, so the
//! server validated the PTY against its own cwd and rejected the upgrade
//! (403/404) whenever the app was launched from another directory. The tab
//! then sat on "Connecting to shell..." forever with nowhere to type.
//!
//! This test walks EXACTLY the desktop path — `EmbeddedEngine` +
//! `create_pty` + `pty_ticket` + `pty_ws_request` + `connect_async` — and
//! requires a 101 upgrade plus a live echo round-trip.

use code_plus_plus_desktop::terminal_vscode::{parse_agent_pty_id, pty_ws_request};
use futures::{SinkExt, StreamExt};

#[tokio::test]
async fn pty_ws_full_duplex_with_directory_header() {
    let project_dir = env!("CARGO_MANIFEST_DIR").to_string();
    let engine = code_plus_plus_sdk::EmbeddedEngine::start(project_dir)
        .await
        .expect("embedded engine starts");
    let client = engine.client();

    let pty_id = client
        .create_pty(Some(24), Some(80))
        .await
        .expect("create_pty succeeds");
    let ticket = client
        .pty_ticket(&pty_id)
        .await
        .map(|(t, _)| t)
        .unwrap_or_default();
    let mut path = format!("/api/v1/pty/{pty_id}/connect");
    if !ticket.is_empty() {
        path.push_str(&format!("?ticket={ticket}"));
    }
    // Must upgrade (101): any 403/404 here is the original bug resurfacing.
    let request = pty_ws_request(&client.ws_url(&path), client.directory());
    let (ws_stream, _) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("handshake completes in time")
    .expect("handshake succeeds (101 upgrade)");

    // The shell must be live: echo a probe and read it back.
    let (mut write, mut read) = ws_stream.split();
    write
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            b"echo PROBE_HANDSHAKE_OK\r".to_vec().into(),
        ))
        .await
        .expect("write reaches the shell");
    let mut collected = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline - std::time::Instant::now();
        let next = tokio::time::timeout(remaining, read.next()).await;
        let Some(frame) = next.ok().flatten() else {
            break;
        };
        let frame = frame.expect("readable ws frame");
        match frame {
            tokio_tungstenite::tungstenite::Message::Binary(data) => {
                collected.push_str(&String::from_utf8_lossy(&data));
            }
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                collected.push_str(&text);
            }
            _ => break,
        }
        if collected.contains("PROBE_HANDSHAKE_OK") {
            break;
        }
    }
    assert!(
        collected.contains("PROBE_HANDSHAKE_OK"),
        "shell echoed the probe; got: {collected:?}"
    );
    engine.shutdown().await;
}

/// Agent PTY path: the tool spawns into the core registry, the desktop
/// attaches over `/api/v1/agent-pty/{id}/connect` with the SAME handshake
/// helper, and bytes flow both ways. The `stream == "pty"` chunk shape the
/// tool emits must parse here.
#[tokio::test]
async fn agent_pty_attach_full_duplex() {
    assert_eq!(
        parse_agent_pty_id(r#"{"pty_id":"01ABC"}"#),
        Some("01ABC".to_string())
    );

    let project_dir = env!("CARGO_MANIFEST_DIR").to_string();
    let engine = code_plus_plus_sdk::EmbeddedEngine::start(project_dir.clone())
        .await
        .expect("embedded engine starts");
    let client = engine.client();

    // Same spawn the bash tool performs (argv, cwd): short-lived by design.
    let (program, args): (String, Vec<String>) = if cfg!(windows) {
        (
            "cmd".to_string(),
            vec![
                "/C".to_string(),
                "echo AGENT_PROBE_OK".to_string(),
            ],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "echo AGENT_PROBE_OK".to_string()],
        )
    };
    let (session, mut child, _rx) =
        code_plus_plus_core::tools::agent_pty::global_agent_pty_registry()
            .spawn(&program, &args, &project_dir, &[])
            .expect("agent pty spawn works");
    let path = format!("/api/v1/agent-pty/{}/connect", session.id);
    let request = pty_ws_request(&client.ws_url(&path), client.directory());
    let (ws_stream, _) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("agent handshake completes in time")
    .expect("agent handshake succeeds (101 upgrade)");

    let (mut write, mut read) = ws_stream.split();
    // Interactive direction: keystrokes must reach the session.
    write
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            b"\r".to_vec().into(),
        ))
        .await
        .expect("write reaches the agent pty");
    let mut collected = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline - std::time::Instant::now();
        let next = tokio::time::timeout(remaining, read.next()).await;
        let Some(frame) = next.ok().flatten() else {
            break;
        };
        let frame = frame.expect("readable ws frame");
        match frame {
            tokio_tungstenite::tungstenite::Message::Binary(data) => {
                collected.push_str(&String::from_utf8_lossy(&data));
            }
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                collected.push_str(&text);
            }
            _ => break,
        }
        if collected.contains("AGENT_PROBE_OK") {
            break;
        }
    }
    let _ = child.wait();
    code_plus_plus_core::tools::agent_pty::global_agent_pty_registry().remove(&session.id);
    assert!(
        collected.contains("AGENT_PROBE_OK"),
        "agent pty streamed the command output; got: {collected:?}"
    );
    engine.shutdown().await;
}
