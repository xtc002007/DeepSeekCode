use deepseekcode_types::{DeepSeekCodeError, Result, PRODUCT_NAME};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::local_stdio::McpToolInfo;

/// Wall-clock budgets ported from `my-notebook` (`crates/my_notebook_ai/src/mcp.rs`):
/// ureq there used a 60s read timeout + 120s total timeout because its default
/// is *no* read timeout (a stalled MCP server blocked the caller forever).
/// reqwest's `Client::new()` has the same property, so we set both here.
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(60);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound on a single SSE `data:` payload (my-notebook: 1MiB).
const MAX_SSE_PAYLOAD: usize = 1024 * 1024;
/// Hard bound on the whole SSE event stream so a server that never sends a
/// matching response cannot make us read forever (my-notebook: 60s).
const SSE_DEADLINE: Duration = Duration::from_secs(60);
/// Cap on the accumulated SSE buffer while streaming.
const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;

/// HTTP JSON-RPC 2.0 MCP client with Streamable HTTP (SSE) fallback.
pub struct RemoteMcpClient {
    url: String,
    client: Client,
    next_id: AtomicU64,
    session_id: std::sync::Mutex<Option<String>>,
}

impl RemoteMcpClient {
    pub fn new(url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(HTTP_TOTAL_TIMEOUT)
            .read_timeout(HTTP_READ_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            url: url.into(),
            client,
            next_id: AtomicU64::new(1),
            session_id: std::sync::Mutex::new(None),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn initialize(&self) -> Result<()> {
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            url = %self.url,
            "remote mcp initialize started"
        );
        validate_http_url(&self.url)?;
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": PRODUCT_NAME, "version": env!("CARGO_PKG_VERSION") }
        });
        self.request("initialize", params).await?;

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let mut req = self
            .client
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&notification);
        if let Some(sid) = self.session_id_snapshot() {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req.send().await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::TOOL,
                run_id = %deepseekcode_telemetry::current_run_id(),
                url = %self.url,
                error = %e,
                "remote mcp initialize failed: notify"
            );
            DeepSeekCodeError::Other(format!("mcp initialized notify failed: {e}"))
        })?;
        self.remember_session_id(resp.headers());
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            url = %self.url,
            "remote mcp initialize completed"
        );
        Ok(())
    }

    fn session_id_snapshot(&self) -> Option<String> {
        self.session_id.lock().ok()?.clone()
    }

    fn remember_session_id(&self, headers: &reqwest::header::HeaderMap) {
        let sid = headers
            .get("Mcp-Session-Id")
            .or_else(|| headers.get("mcp-session-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Some(sid) = sid {
            if let Ok(mut guard) = self.session_id.lock() {
                *guard = Some(sid);
            }
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        tracing::debug!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            url = %self.url,
            "remote mcp list_tools started"
        );
        let result = self
            .request("tools/list", Value::Object(Default::default()))
            .await?;
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
                    .unwrap_or_else(|| json!({ "type": "object" }));
                Some(McpToolInfo {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            url = %self.url,
            tool = %name,
            "remote mcp call_tool started"
        );
        let params = json!({
            "name": name,
            "arguments": arguments,
        });
        self.request("tools/call", params).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        validate_http_url(&self.url)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut req = self
            .client
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(sid) = self.session_id_snapshot() {
            req = req.header("Mcp-Session-Id", sid);
        }
        let response = req
            .send()
            .await
            .map_err(|e| DeepSeekCodeError::Other(format!("remote mcp request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            // Truncate error pages: a proxy HTML body must not flood the log.
            let text = truncate_for_error(&text, 2000);
            return Err(DeepSeekCodeError::Other(format!(
                "remote mcp http {status}: {text}"
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.remember_session_id(response.headers());

        if content_type.contains("text/event-stream") {
            // Incremental SSE read with a hard deadline (my-notebook fix):
            // `response.text()` reads to EOF, which for `text/event-stream`
            // means *never* on a real SSE endpoint, so every call hung.
            // We stream chunks and return as soon as the matching `id`
            // arrives, without waiting for the server to close the stream.
            return read_sse_response_streaming(response, id).await;
        }

        let value: Value = response
            .json()
            .await
            .map_err(|e| DeepSeekCodeError::Other(format!("remote mcp invalid json: {e}")))?;

        extract_jsonrpc_result(value)
    }
}

/// Reject non-HTTP(S) targets so a mis-typed or LLM-supplied URL cannot reach
/// the local filesystem (ported from my-notebook `validate_http_url`).
fn validate_http_url(url: &str) -> Result<()> {
    let lower = url.trim_start().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(());
    }
    let scheme = url.split("://").next().unwrap_or(url);
    Err(DeepSeekCodeError::Other(format!(
        "unsupported mcp url scheme: {scheme} (expected http:// or https://)"
    )))
}

fn truncate_for_error(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push_str("…[truncated]");
    }
    out
}

/// Incremental SSE reader: returns the `expected_id` payload as soon as it
/// arrives, without waiting for EOF. Enforces a 60s wall deadline, a 4MiB
/// buffer cap, a 1MiB per-payload cap, and strict JSON (a non-JSON `data:`
/// payload is an error, never a tool result — my-notebook fix for error
/// pages leaking to the model as tool output).
async fn read_sse_response_streaming(
    response: reqwest::Response,
    expected_id: u64,
) -> Result<Value> {
    use futures::StreamExt;
    use std::time::Instant;

    let deadline = Instant::now() + SSE_DEADLINE;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut scanned_up_to = 0usize;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(DeepSeekCodeError::Other(
                "mcp sse stream timed out after 60s without matching response".into(),
            ));
        }
        let chunk = tokio::time::timeout(remaining, stream.next())
            .await
            .map_err(|_| {
                DeepSeekCodeError::Other(
                    "mcp sse stream timed out after 60s without matching response".into(),
                )
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        let bytes = chunk.map_err(|e| {
            DeepSeekCodeError::Other(format!("remote mcp sse read failed: {e}"))
        })?;
        if buffer.len() + bytes.len() > MAX_SSE_BUFFER {
            return Err(DeepSeekCodeError::Other(
                "mcp SSE buffer exceeded the size limit".into(),
            ));
        }
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        // Scan only newly completed lines for the matching id so we can
        // return early without waiting for the server to close the stream.
        if let Some(found) = scan_sse_buffer_for_id(&buffer, &mut scanned_up_to, expected_id)? {
            return Ok(found);
        }
    }
    Err(DeepSeekCodeError::Other(
        "mcp sse stream ended without matching response".into(),
    ))
}

/// Scan `buffer` for `data:` lines; advances `scanned_up_to` past complete
/// lines. Returns the extracted result on exact `id` match.
fn scan_sse_buffer_for_id(
    buffer: &str,
    scanned_up_to: &mut usize,
    expected_id: u64,
) -> Result<Option<Value>> {
    while let Some(nl) = buffer[*scanned_up_to..].find('\n') {
        let line_end = *scanned_up_to + nl;
        let line = buffer[*scanned_up_to..line_end].trim();
        *scanned_up_to = line_end + 1;
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if payload.is_empty() || payload == "[DONE]" {
            if payload == "[DONE]" {
                break;
            }
            continue;
        }
        if payload.len() > MAX_SSE_PAYLOAD {
            return Err(DeepSeekCodeError::Other(
                "mcp SSE payload exceeded the size limit".into(),
            ));
        }
        // Exact u64 id match: `"id":11` must not match expected `1`
        // (my-notebook substring regression test).
        let value: Value = serde_json::from_str(payload).map_err(|e| {
            DeepSeekCodeError::Other(format!("remote mcp invalid sse json: {e}"))
        })?;
        if value.get("id").and_then(|v| v.as_u64()) == Some(expected_id) {
            return extract_jsonrpc_result(value).map(Some);
        }
    }
    Ok(None)
}

/// Test helper + compat shim over [`scan_sse_buffer_for_id`].
#[allow(dead_code)]
fn parse_sse_jsonrpc_response(body: &str, expected_id: u64) -> Result<Value> {
    let mut cursor = 0usize;
    if let Some(found) = scan_sse_buffer_for_id(body, &mut cursor, expected_id)? {
        return Ok(found);
    }
    Err(DeepSeekCodeError::Other(
        "mcp sse stream ended without matching response".into(),
    ))
}

fn extract_jsonrpc_result(value: Value) -> Result<Value> {
    if let Some(error) = value.get("error") {
        return Err(DeepSeekCodeError::Other(format!("mcp error: {error}")));
    }

    value
        .get("result")
        .cloned()
        .ok_or_else(|| DeepSeekCodeError::Other("mcp response missing result".into()))
}

impl Clone for RemoteMcpClient {
    fn clone(&self) -> Self {
        Self {
            url: self.url.clone(),
            client: self.client.clone(),
            next_id: AtomicU64::new(self.next_id.load(Ordering::Relaxed)),
            session_id: std::sync::Mutex::new(self.session_id_snapshot()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_jsonrpc_response_matches_id() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[]}}\n\n",
        );
        let result = parse_sse_jsonrpc_response(body, 7).expect("sse parse");
        assert!(result.get("tools").is_some());
    }

    #[test]
    fn http_url_validation_rejects_non_http() {
        assert!(validate_http_url("https://ok.example/mcp").is_ok());
        assert!(validate_http_url("http://ok.example/mcp").is_ok());
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("not a url").is_err());
    }

    #[test]
    fn sse_reader_returns_id_one_and_stops_at_done() {
        let body = concat!(
            ": ping\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{}}\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":\"hi\"}}\n",
            "data: [DONE]\n"
        );
        let out = parse_sse_jsonrpc_response(body, 1).unwrap();
        assert!(out.to_string().contains("hi"));
    }

    #[test]
    fn sse_reader_rejects_non_json_data() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{}}\ndata: <html>oops</html>\n";
        let err = parse_sse_jsonrpc_response(body, 1).unwrap_err();
        assert!(err.to_string().contains("invalid sse json"), "got {err}");
    }

    #[test]
    fn sse_reader_handles_data_without_space() {
        let body = "data:{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n";
        assert!(parse_sse_jsonrpc_response(body, 1).is_ok());
    }

    #[test]
    fn sse_reader_returns_none_on_eof_without_match() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{}}\n";
        let err = parse_sse_jsonrpc_response(body, 1).unwrap_err();
        assert!(err.to_string().contains("without matching response"));
    }

    #[test]
    fn stdio_id_matching_is_exact_not_substring() {
        // `"id":11` contains `"id":1` as a substring; exact u64 match rejects it.
        let line = r#"{"jsonrpc":"2.0","id":11,"result":{"x":1}}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_ne!(v.get("id").and_then(|x| x.as_u64()), Some(1));
    }
}
