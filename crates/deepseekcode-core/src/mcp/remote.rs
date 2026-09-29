use deepseekcode_types::{DeepSeekCodeError, Result, PRODUCT_NAME};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

use super::local_stdio::McpToolInfo;

/// HTTP JSON-RPC 2.0 MCP client with Streamable HTTP (SSE) fallback.
pub struct RemoteMcpClient {
    url: String,
    client: Client,
    next_id: AtomicU64,
}

impl RemoteMcpClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            client: Client::new(),
            next_id: AtomicU64::new(1),
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
        self.client
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&notification)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::TOOL,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    url = %self.url,
                    error = %e,
                    "remote mcp initialize failed: notify"
                );
                DeepSeekCodeError::Other(format!("mcp initialized notify failed: {e}"))
            })?;
        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            url = %self.url,
            "remote mcp initialize completed"
        );
        Ok(())
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response = self
            .client
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| DeepSeekCodeError::Other(format!("remote mcp request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
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

        if content_type.contains("text/event-stream") {
            let text = response.text().await.map_err(|e| {
                DeepSeekCodeError::Other(format!("remote mcp sse read failed: {e}"))
            })?;
            return parse_sse_jsonrpc_response(&text, id);
        }

        let value: Value = response
            .json()
            .await
            .map_err(|e| DeepSeekCodeError::Other(format!("remote mcp invalid json: {e}")))?;

        extract_jsonrpc_result(value)
    }
}

fn parse_sse_jsonrpc_response(body: &str, expected_id: u64) -> Result<Value> {
    for line in body.lines() {
        let payload = line.strip_prefix("data:").map(str::trim);
        let Some(payload) = payload else {
            continue;
        };
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(payload)
            .map_err(|e| DeepSeekCodeError::Other(format!("remote mcp invalid sse json: {e}")))?;
        if value.get("id").and_then(|v| v.as_u64()) == Some(expected_id) {
            return extract_jsonrpc_result(value);
        }
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
}
