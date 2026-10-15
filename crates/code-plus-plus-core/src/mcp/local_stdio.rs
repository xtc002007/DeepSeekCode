use code_plus_plus_types::{CodePlusPlusError, Result, PRODUCT_NAME};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct LocalMcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl LocalMcpClient {
    pub fn spawn(command: &str, args: &[String]) -> Result<Self> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            command = %command,
            args = args.len(),
            "local mcp spawn started"
        );
        let mut cmd = Command::new(command);
        crate::process::hide_console(&mut cmd);
        let mut child = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piped (not null) so child errors are not silently lost; the
            // handle is drained on Drop to avoid pipe-buffer deadlock.
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                tracing::error!(
                    target: code_plus_plus_telemetry::TOOL,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    command = %command,
                    error = %e,
                    "local mcp spawn failed"
                );
                CodePlusPlusError::Other(format!("mcp spawn failed: {e}"))
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CodePlusPlusError::Other("mcp stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CodePlusPlusError::Other("mcp stdout unavailable".into()))?;
        // Drain stderr in the background: a piped-but-unread stderr blocks
        // the child once the OS pipe buffer fills, hanging all requests.
        // Bounded log capture (last 8KiB) keeps server errors diagnosable.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut tail = Vec::new();
                let mut buf = [0u8; 4096];
                let mut reader = stderr;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            tail.extend_from_slice(&buf[..n]);
                            if tail.len() > 8192 {
                                let excess = tail.len() - 8192;
                                tail.drain(..excess);
                            }
                        }
                        Err(_) => break,
                    }
                }
                if !tail.is_empty() {
                    tracing::debug!(
                        target: code_plus_plus_telemetry::APP,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        stderr = %String::from_utf8_lossy(&tail),
                        "mcp server stderr closed"
                    );
                }
            });
        }

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub fn initialize(&mut self) -> Result<()> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "local mcp initialize started"
        );
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": PRODUCT_NAME, "version": env!("CARGO_PKG_VERSION") }
        });
        self.request("initialize", params)?;

        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        writeln!(self.stdin, "{notification}").map_err(CodePlusPlusError::Io)?;
        self.stdin.flush().map_err(CodePlusPlusError::Io)?;
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "local mcp list_tools started"
        );
        let result = self.request("tools/list", Value::Object(Default::default()))?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(tools
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_string();
                let description = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
                Some(McpToolInfo {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect())
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            tool = %name,
            "local mcp call_tool started"
        );
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });
        self.request("tools/call", params)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        use std::time::{Duration, Instant};
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").map_err(CodePlusPlusError::Io)?;
        self.stdin.flush().map_err(CodePlusPlusError::Io)?;

        // Wall deadline + notification skipping: lines without matching `id`
        // no longer consume a fixed 50-line budget, and EOF (child exit) is
        // distinguished from timeout (P0-4). NOTE: `read_line` blocks; the
        // deadline is enforced between lines and via child-exit detection.
        // A fully silent server is additionally bounded by `connect` timeout
        // (15s) and the drain watchdog.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut skipped_notifications = 0u32;
        loop {
            if Instant::now() > deadline {
                return Err(CodePlusPlusError::Other(format!(
                    "mcp request timed out after 30s: {method}"
                )));
            }
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(CodePlusPlusError::Io)?;
            if read == 0 {
                // EOF: child exited or closed stdout.
                match self.child.try_wait() {
                    Ok(Some(status)) => {
                        return Err(CodePlusPlusError::Other(format!(
                            "mcp server exited ({status}) before responding: {method}"
                        )))
                    }
                    _ => {
                        return Err(CodePlusPlusError::Other(format!(
                            "mcp request EOF without response: {method}"
                        )))
                    }
                }
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => {
                    // Non-JSON log line from the server: skip, do not count.
                    skipped_notifications += 1;
                    if skipped_notifications > 1000 {
                        return Err(CodePlusPlusError::Other(format!(
                            "mcp server sent too many non-JSON lines: {method}"
                        )));
                    }
                    continue;
                }
            };
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(CodePlusPlusError::Other(format!("mcp error: {error}")));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| CodePlusPlusError::Other("mcp response missing result".into()));
            }
            // Notification / log without matching id: skip without budget.
            skipped_notifications += 1;
            if skipped_notifications > 1000 {
                return Err(CodePlusPlusError::Other(format!(
                    "mcp server sent too many notifications without response: {method}"
                )));
            }
        }
    }
}

impl Drop for LocalMcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
