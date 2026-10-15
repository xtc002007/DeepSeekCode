//! VSCode-aligned terminal helpers: prompt parsing + bash result parsing.
//!
//! Extracted from `terminal.rs` / `activity_click_router.rs` so both the
//! interactive PTY view and the read-only bash-tool view share one palette
//! and one input/output model.





/// Build the PTY WebSocket handshake request carrying the project directory.
///
/// `connect_async(url)` sends no headers, so the server falls back to its
/// own cwd and `check_ownership` rejects the PTY whenever the app was
/// launched from a different directory (403/404 → a shell that connects to
/// nothing and can never receive input). The explicit header keeps the
/// handshake project-scoped; the server additionally honors `?directory=`.
pub fn pty_ws_request(ws_url: &str, directory: &str) -> http::Request<()> {
    let host = ws_url
        .split("://")
        .nth(1)
        .unwrap_or(ws_url)
        .split('/')
        .next()
        .unwrap_or("");
    // A hand-built request bypasses tungstenite's URL-mode header synthesis,
    // so the RFC 6455 handshake headers must be set explicitly — without
    // `Sec-WebSocket-Key` the client itself aborts with
    // `InvalidHeader("sec-websocket-key")` before a byte hits the wire.
    // (`Ulid` supplies the 128-bit nonce; it is already a dependency.)
    use base64::Engine as _;
    let nonce = base64::engine::general_purpose::STANDARD.encode(ulid::Ulid::new().0.to_be_bytes());
    http::Request::builder()
        .uri(ws_url)
        .header("host", host)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", nonce)
        .header("x-code-plus-plus-directory", directory)
        .body(())
        .expect("pty websocket request is well-formed")
}

/// Build the replay payload for a history shell: `{cwd}> {command}` echo
/// line plus the recorded stdout/stderr, as raw terminal bytes. A fresh
/// interactive shell renders these through its grid BEFORE its own banner
/// and prompt arrive, so the tab reads exactly like the original session —
/// and stays fully typable afterwards. No exit-code footers: a real console
/// never prints those.
pub fn build_replay_bytes(cwd: &str, command: &str, stdout: &str, stderr: &str) -> Vec<u8> {
    let mut text = String::new();
    let prompt = cwd.trim();
    if prompt.is_empty() {
        text.push_str("$ ");
    } else {
        text.push_str(prompt);
        text.push_str("> ");
    }
    text.push_str(command.trim_end());
    text.push_str("\r\n");
    if !stdout.is_empty() {
        text.push_str(stdout.trim_end_matches(['\r', '\n']));
        text.push_str("\r\n");
    }
    if !stderr.trim().is_empty() {
        text.push_str(stderr.trim_end_matches(['\r', '\n']));
        text.push_str("\r\n");
    }
    text.into_bytes()
}

/// Parse the agent-PTY attach handshake out of a `stream == "pty"` progress
/// chunk (`{"pty_id": "…"}`). Anything else yields `None`.
pub fn parse_agent_pty_id(chunk: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(chunk)
        .ok()?
        .get("pty_id")?
        .as_str()
        .map(str::to_string)
        .filter(|id| !id.trim().is_empty())
}

/// Best-effort command line for a bash transcript: the captured `command`,
/// else the `command`/`script` field inside the raw tool-args JSON, else the
/// first non-empty line of that JSON. Never returns whitespace-only text.
pub fn transcript_command(
    command: Option<&str>,
    input_json: Option<&str>,
) -> Option<String> {
    if let Some(cmd) = command.map(str::trim).filter(|c| !c.is_empty()) {
        return Some(cmd.to_string());
    }
    let raw = input_json?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        for key in ["command", "script"] {
            if let Some(cmd) = v
                .get(key)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                return Some(cmd.to_string());
            }
        }
    }
    raw.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Structured bash tool result parsed from `DetailPayload.output`.
///
/// `output` may be:
/// - streaming JSON `{"stdout": "..", "stderr": ".."}`,
/// - completed JSON `{"stdout":.., "stderr":.., "exit_code":.., "success":..}`
///   or `{"output": {...}}`,
/// - legacy plain display text (already merged `$ cmd` stripped).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i64>,
    pub success: Option<bool>,
    pub error: Option<String>,
    pub truncated: bool,
}

impl BashResult {
    pub fn is_empty(&self) -> bool {
        self.stdout.trim().is_empty() && self.stderr.trim().is_empty() && self.error.is_none()
    }
}

fn get_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

pub fn parse_bash_result(raw: &str) -> BashResult {
    let mut result = BashResult::default();
    if raw.trim().is_empty() {
        return result;
    }
    // Try streaming / completed JSON first.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        // Unwrap `{"output": {...}}` envelope used by ToolCompleted payloads.
        let body = v.get("output").unwrap_or(&v);
        let mut found = false;
        if let Some(s) = body.get("stdout").and_then(|x| x.as_str()) {
            result.stdout = s.to_string();
            found = true;
        }
        if let Some(s) = body.get("stderr").and_then(|x| x.as_str()) {
            result.stderr = s.to_string();
            found = true;
        }
        if let Some(c) = body.get("exit_code").and_then(|x| x.as_i64()) {
            result.exit_code = Some(c);
            found = true;
        }
        if let Some(s) = body.get("success").and_then(|x| x.as_bool()) {
            result.success = Some(s);
            found = true;
        }
        if let Some(e) = body.get("error").and_then(|x| x.as_str()) {
            if !e.is_empty() {
                result.error = Some(format!("Error: {e}"));
                found = true;
            }
        }
        if let Some(t) = body.get("truncated").and_then(|x| x.as_bool()) {
            result.truncated = t;
            found = true;
        }
        if let Some(content) = body.get("content").and_then(|x| x.as_str()) {
            result.stdout = content.to_string();
            found = true;
        }
        if found {
            return result;
        }
        // `{"stdout":..,"stderr":..}` at top level (stream body).
        if let Some(s) = get_str(&v, "stdout") {
            result.stdout = s.to_string();
            found = true;
        }
        if let Some(s) = get_str(&v, "stderr") {
            result.stderr = s.to_string();
            found = true;
        }
        if found {
            return result;
        }
    }
    // Plain text fallback: strip `[stdout] `/`[stderr] ` prefixes, split
    // stderr-looking lines? Keep everything as stdout — stderr separation is
    // only available from JSON.
    let text: String = raw
        .lines()
        .map(|line| {
            line.strip_prefix("[stdout] ")
                .or_else(|| line.strip_prefix("[stderr] "))
                .unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    result.stdout = text;
    result
}

/// Merge streamed stdout/stderr with the completed tool output into a single
/// JSON document preserving `exit_code`/`success`/`error`.
///
/// This is what bash terminal tabs persist: the renderer derives the display
/// text via [`parse_bash_result`], so input/output stay inspectable.
pub fn merge_bash_result_json(
    streamed: Option<&str>,
    completed_value: &serde_json::Value,
) -> String {
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(raw) = streamed {
        let partial = parse_bash_result(raw);
        stdout = partial.stdout;
        stderr = partial.stderr;
    }
    let out = completed_value
        .get("output")
        .filter(|v| !v.is_null())
        .unwrap_or(completed_value);
    if let Some(s) = out.get("stdout").and_then(|v| v.as_str()) {
        if stdout.is_empty() {
            stdout = s.to_string();
        }
    }
    if let Some(s) = out.get("stderr").and_then(|v| v.as_str()) {
        if stderr.is_empty() {
            stderr = s.to_string();
        }
    }
    // Fall back to plain-text display extraction when the tool returned no
    // structured stdout (keeps legacy behaviour for non-bash-shaped output).
    if stdout.is_empty() && stderr.is_empty() {
        let fallback = crate::workspace::activity_click_router::extract_bash_display_text(
            &serde_json::to_string(completed_value)
                .unwrap_or_else(|_| completed_value.to_string()),
        );
        if !fallback.trim().is_empty() {
            stdout = fallback;
        }
    }
    let exit_code = out.get("exit_code").and_then(|v| v.as_i64());
    let success = out.get("success").and_then(|v| v.as_bool());
    let error = out.get("error").and_then(|v| v.as_str()).map(String::from);
    let truncated = out
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
        "success": success,
        "error": error,
        "truncated": truncated,
    })
    .to_string()
}

/// Best-effort PowerShell prompt segmentation for the echo line.
///
/// `PS D:\Workspace\Digest> dir` → (`PS D:\Workspace\Digest>`, `dir`).
/// Returns `(None, line)` when the line is not a recognised prompt.
pub fn split_ps_prompt(line: &str) -> (Option<String>, String) {
    let trimmed = line.trim_end();
    // PowerShell: `PS <path>> <cmd>` / `PS <path>>` (no cmd yet).
    if let Some(rest) = trimmed.strip_prefix("PS ") {
        if let Some(idx) = rest.find('>') {
            let (path, after) = rest.split_at(idx + 1);
            let prompt = format!("PS {path}");
            let cmd = after.trim_start().to_string();
            return (Some(prompt), cmd);
        }
    }
    // sh/bash: `$ <cmd>`, `# <cmd>`, `❯ <cmd>`.
    for prefix in ["$ ", "# ", "❯ "] {
        if let Some(cmd) = trimmed.strip_prefix(prefix) {
            return (
                Some(prefix.trim_end().to_string()),
                cmd.to_string(),
            );
        }
    }
    (None, trimmed.to_string())
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_json() {
        let r = parse_bash_result(r#"{"stdout":"ok\n","stderr":"warn\n"}"#);
        assert_eq!(r.stdout, "ok\n");
        assert_eq!(r.stderr, "warn\n");
    }

    #[test]
    fn parses_completed_envelope_with_exit_code() {
        let raw = serde_json::json!({
            "output": {"stdout": "hi", "stderr": "", "exit_code": 0, "success": true}
        })
        .to_string();
        let r = parse_bash_result(&raw);
        assert_eq!(r.stdout, "hi");
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.success, Some(true));
    }

    #[test]
    fn merge_preserves_stream_and_exit_code() {
        let streamed = r#"{"stdout":"partial\n","stderr":""}"#;
        let completed = serde_json::json!({
            "output": {"error": "command timed out after 60000ms", "success": false}
        });
        let merged = merge_bash_result_json(Some(streamed), &completed);
        let r = parse_bash_result(&merged);
        assert!(r.stdout.contains("partial"), "{r:?}");
        assert_eq!(r.success, Some(false));
        assert!(r.error.as_deref().unwrap_or("").contains("timed out"), "{r:?}");
    }

    #[test]
    fn splits_powershell_prompt() {
        let (prompt, cmd) = split_ps_prompt("PS D:\\Workspace\\Digest> dir");
        assert_eq!(prompt.as_deref(), Some("PS D:\\Workspace\\Digest>"));
        assert_eq!(cmd, "dir");
    }

    #[test]
    fn splits_dollar_prompt() {
        let (prompt, cmd) = split_ps_prompt("$ cargo test");
        assert_eq!(prompt.as_deref(), Some("$"));
        assert_eq!(cmd, "cargo test");
    }

    #[test]
    fn plain_output_falls_back_to_stdout() {
        let r = parse_bash_result("hello\nworld");
        assert_eq!(r.stdout, "hello\nworld");
        assert!(r.stderr.is_empty());
    }

    #[test]
    fn ws_request_carries_host_and_directory() {
        // 回归：之前 `connect_async(url)` 不带目录头，server 用自身 cwd
        // 做 ownership 校验，目录不一致直接 403/404，Shell 永远连不上。
        let req = super::pty_ws_request(
            "ws://127.0.0.1:4096/api/v1/pty/abc/connect?ticket=t",
            "D:\\Workspace\\Echoo",
        );
        assert_eq!(req.uri().to_string(), "ws://127.0.0.1:4096/api/v1/pty/abc/connect?ticket=t");
        assert_eq!(req.headers()["host"], "127.0.0.1:4096");
        assert_eq!(
            req.headers()["x-code-plus-plus-directory"],
            "D:\\Workspace\\Echoo"
        );
        // RFC 6455 headers must be present: tungstenite does NOT synthesize
        // them for a pre-built `Request` (missing key = instant client-side
        // `InvalidHeader("sec-websocket-key")`).
        assert_eq!(req.headers()["upgrade"], "websocket");
        assert_eq!(req.headers()["sec-websocket-version"], "13");
        let key = req.headers()["sec-websocket-key"].to_str().unwrap();
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(key)
            .expect("key is base64");
        assert_eq!(raw.len(), 16, "128-bit nonce");
    }

    #[test]
    fn replay_bytes_read_like_a_console_session() {
        let bytes = super::build_replay_bytes(
            "D:\\Workspace\\Echoo",
            "dir /b apps",
            "mobile\nserver\n",
            "",
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("D:\\Workspace\\Echoo> dir /b apps\r\n"), "{text:?}");
        assert!(text.contains("mobile"), "{text:?}");
        assert!(!text.contains("exit code"), "{text:?}");
    }

    #[test]
    fn replay_bytes_handles_empty_command_and_stderr() {
        let bytes = super::build_replay_bytes("", "(unknown command)", "", "boom\n");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("$ (unknown command)\r\n"), "{text:?}");
        assert!(text.contains("boom"), "{text:?}");
    }

    #[test]
    fn agent_pty_chunk_parses_session_id() {
        assert_eq!(
            super::parse_agent_pty_id(r#"{"pty_id":"01ABC"}"#),
            Some("01ABC".to_string())
        );
        assert_eq!(super::parse_agent_pty_id(r#"{"stdout":"hi"}"#), None);
        assert_eq!(super::parse_agent_pty_id("not json"), None);
        assert_eq!(super::parse_agent_pty_id(r#"{"pty_id":""}"#), None);
    }

    #[test]
    fn transcript_command_prefers_captured_then_input_json() {
        assert_eq!(
            super::transcript_command(Some("dir /b"), Some("{\"command\":\"x\"}")),
            Some("dir /b".to_string())
        );
        // 会话切换后 detail 重建丢了 command，但 input JSON 还在。
        assert_eq!(
            super::transcript_command(None, Some("{\n  \"command\": \"dir /b\"\n}")),
            Some("dir /b".to_string())
        );
        assert_eq!(
            super::transcript_command(Some("   "), Some("{\"script\":\"run.ps1\"}")),
            Some("run.ps1".to_string())
        );
        assert_eq!(super::transcript_command(None, None), None);
        assert_eq!(super::transcript_command(Some("  "), Some("   ")), None);
    }
}
