use deepseekcode_types::{DeepSeekCodeError, Result, PRODUCT_NAME};
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
        let mut cmd = Command::new(command);
        crate::process::hide_console(&mut cmd);
        let mut child = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DeepSeekCodeError::Other(format!("mcp spawn failed: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DeepSeekCodeError::Other("mcp stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DeepSeekCodeError::Other("mcp stdout unavailable".into()))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub fn initialize(&mut self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": PRODUCT_NAME, "version": "0.1.0" }
        });
        self.request("initialize", params)?;

        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        writeln!(self.stdin, "{notification}").map_err(DeepSeekCodeError::Io)?;
        self.stdin.flush().map_err(DeepSeekCodeError::Io)?;
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>> {
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
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });
        self.request("tools/call", params)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").map_err(DeepSeekCodeError::Io)?;
        self.stdin.flush().map_err(DeepSeekCodeError::Io)?;

        for _ in 0..50 {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(DeepSeekCodeError::Io)?;
            if read == 0 {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(line).map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(DeepSeekCodeError::Other(format!("mcp error: {error}")));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| DeepSeekCodeError::Other("mcp response missing result".into()));
            }
        }
        Err(DeepSeekCodeError::Other(format!(
            "mcp request timed out: {method}"
        )))
    }
}

impl Drop for LocalMcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
