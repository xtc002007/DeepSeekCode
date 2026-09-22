//! Issue-7: stable `use_capability` proxy for MCP.
//!
//! Directly bridging every MCP tool (`mcp_srv_tool`) rewrites the provider
//! `tools` array on each connect/disconnect — a full DeepSeek prefix-cache
//! miss. This single tool has a frozen schema; MCP catalog churn travels in
//! tool *results* (the tail), never in the prefix.

use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::mcp::{McpConnectionPool, McpServerStatus};

/// Catalog fingerprint: sha256 over sorted `server|tool` pairs, first 16 hex
/// chars. Embedded in `list` cursors (`fingerprint:offset`) so a catalog that
/// moved under the caller fails loudly instead of skipping entries.
fn catalog_fingerprint(status: &[McpServerStatus]) -> String {
    use sha2::{Digest, Sha256};
    let mut pairs: Vec<String> = Vec::new();
    for s in status {
        let mut tools = s.tools.clone();
        tools.sort();
        for t in tools {
            pairs.push(format!("{}|{t}", s.name));
        }
    }
    pairs.sort();
    let mut hasher = Sha256::new();
    for p in &pairs {
        hasher.update(p.as_bytes());
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())[..16].to_string()
}

/// Parse a `fingerprint:offset` cursor against the current fingerprint.
/// Stale fingerprints and malformed cursors are non-retryable caller errors.
fn parse_list_cursor(cursor: &str, fingerprint: &str) -> std::result::Result<usize, ToolFailure> {
    let invalid = || ToolFailure {
        message: "invalid cursor; re-list without cursor".into(),
        retryable: false,
    };
    let (fp, off) = cursor.rsplit_once(':').ok_or_else(invalid)?;
    if fp != fingerprint {
        return Err(ToolFailure {
            message: "cursor expired: catalog changed; re-list without cursor".into(),
            retryable: false,
        });
    }
    off.parse::<usize>().map_err(|_| invalid())
}

pub struct UseCapabilityTool {
    mcp: Arc<McpConnectionPool>,
}

impl UseCapabilityTool {
    pub fn new(mcp: Arc<McpConnectionPool>) -> Self {
        Self { mcp }
    }
}

#[async_trait]
impl super::registry::Tool for UseCapabilityTool {
    fn name(&self) -> &str {
        "use_capability"
    }

    fn description(&self) -> &str {
        // P1-5: frozen — any edit changes the provider-visible `tools` bytes
        // and cold-misses the DeepSeek prefix. `schema_is_frozen` locks it.
        "Discover and call MCP capabilities without changing the tool contract. Actions: list (servers/tools page), inspect (one tool schema), call (run server/tool with arguments)."
    }

    fn input_schema(&self) -> Value {
        // Frozen core: `action` + its enum + `required` never change (the
        // prefix breaks otherwise — see `schema_is_frozen`). `cursor`/`limit`
        // are optional pagination additions: omitting them preserves the
        // original full-list behavior byte-for-byte.
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "inspect", "call"]},
                "server": {"type": "string"},
                "tool": {"type": "string"},
                "arguments": {"type": "object"},
                "cursor": {"type": "string", "description": "Opaque page cursor from a previous list response"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Max servers per page; omitted means all"}
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match action {
            "list" => {
                // P1-5 paged catalog: `cursor` is `fingerprint:offset`.
                // A changed fingerprint means the catalog moved under the
                // caller — fail loudly (`cursor expired`) instead of silently
                // skipping entries. Omitting both keeps the original full-list
                // shape byte-for-byte.
                let mut status = self.mcp.status();
                status.sort_by(|a, b| a.name.cmp(&b.name));
                let fingerprint = catalog_fingerprint(&status);
                let limit = input
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, 100) as usize);
                let offset = match input.get("cursor").and_then(|v| v.as_str()) {
                    None => 0usize,
                    Some(cursor) => parse_list_cursor(cursor, &fingerprint)?,
                };
                let total = status.len();
                let end = match limit {
                    Some(n) => (offset + n).min(total),
                    None => total,
                };
                let end = end.max(offset.min(total));
                let start = offset.min(total);
                let servers: Vec<Value> = status
                    .iter()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .map(|s| json!({"server": s.name, "connected": s.connected, "tools": s.tools}))
                    .collect();
                let next_cursor = if end < total {
                    Some(format!("{fingerprint}:{end}"))
                } else {
                    None
                };
                Ok(json!({
                    "servers": servers,
                    "fingerprint": fingerprint,
                    "total": total,
                    "next_cursor": next_cursor,
                }))
            }
            "inspect" => {
                let server = input
                    .get("server")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolFailure {
                        message: "inspect requires server".into(),
                        retryable: false,
                    })?;
                let tool =
                    input
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolFailure {
                            message: "inspect requires tool".into(),
                            retryable: false,
                        })?;
                let connected = self.mcp.connected_tools_snapshot();
                for (srv, tools) in connected {
                    if srv == server {
                        for t in tools {
                            if t.name == tool {
                                return Ok(json!({
                                    "server": server,
                                    "tool": t.name,
                                    "description": t.description,
                                    "input_schema": t.input_schema,
                                }));
                            }
                        }
                    }
                }
                Err(ToolFailure {
                    message: format!("capability not found: {server}/{tool}"),
                    retryable: false,
                })
            }
            "call" => {
                let server = input
                    .get("server")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolFailure {
                        message: "call requires server".into(),
                        retryable: false,
                    })?;
                let tool =
                    input
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolFailure {
                            message: "call requires tool".into(),
                            retryable: false,
                        })?;
                let args = input.get("arguments").cloned().unwrap_or(json!({}));
                self.mcp
                    .call_tool(server, tool, args)
                    .await
                    .map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: true,
                    })
            }
            _ => Err(ToolFailure {
                message: "action must be list|inspect|call".into(),
                retryable: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::Tool;

    #[test]
    fn schema_is_frozen() {
        let pool = Arc::new(McpConnectionPool::new(std::collections::HashMap::new()));
        let tool = UseCapabilityTool::new(pool);
        assert_eq!(tool.name(), "use_capability");
        // P1-5: `description` is provider-visible prefix bytes too — lock it
        // alongside the schema, or a copy tweak silently cold-misses.
        assert_eq!(
            tool.description(),
            "Discover and call MCP capabilities without changing the tool contract. Actions: list (servers/tools page), inspect (one tool schema), call (run server/tool with arguments)."
        );
        // Lock the FULL schema value: any added/removed/renamed property
        // changes provider-visible bytes. Optional `cursor`/`limit` below are
        // part of the frozen contract now — extend only with a version bump.
        assert_eq!(
            tool.input_schema(),
            json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "inspect", "call"]},
                    "server": {"type": "string"},
                    "tool": {"type": "string"},
                    "arguments": {"type": "object"},
                    "cursor": {"type": "string", "description": "Opaque page cursor from a previous list response"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Max servers per page; omitted means all"}
                },
                "required": ["action"]
            })
        );
    }

    fn three_server_pool() -> Arc<McpConnectionPool> {
        use deepseekcode_config::schema::McpServerConfig;
        let mut servers = std::collections::HashMap::new();
        for name in ["c-server", "a-server", "b-server"] {
            servers.insert(
                name.to_string(),
                McpServerConfig {
                    command: None,
                    args: None,
                    url: None,
                    enabled: Some(true),
                },
            );
        }
        Arc::new(McpConnectionPool::new(servers))
    }

    fn test_ctx() -> ToolContext {
        use deepseekcode_types::{MessageId, SessionId, ToolCallId};
        ToolContext {
            session_id: SessionId::new(),
            agent: "build".into(),
            assistant_message_id: MessageId::new(),
            tool_call_id: ToolCallId::new(),
            working_directory: "/tmp".into(),
            extras: Default::default(),
        }
    }

    #[tokio::test]
    async fn list_paginates_with_stable_fingerprint() {
        // P1-5: servers sorted by name, pages addressable, fingerprint stable.
        let tool = UseCapabilityTool::new(three_server_pool());
        let ctx = test_ctx();
        let page1 = tool
            .execute(json!({"action": "list", "limit": 2}), &ctx)
            .await
            .unwrap();
        assert_eq!(page1["total"], 3);
        assert_eq!(page1["servers"].as_array().unwrap().len(), 2);
        assert_eq!(page1["servers"][0]["server"], "a-server");
        assert_eq!(page1["servers"][1]["server"], "b-server");
        let fp = page1["fingerprint"].as_str().unwrap().to_string();
        assert_eq!(fp.len(), 16);
        let cursor = page1["next_cursor"].as_str().unwrap().to_string();
        let page2 = tool
            .execute(
                json!({"action": "list", "cursor": cursor, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(page2["servers"].as_array().unwrap().len(), 1);
        assert_eq!(page2["servers"][0]["server"], "c-server");
        assert!(page2["next_cursor"].is_null());
        // Full list (no cursor/limit) keeps the legacy shape plus metadata.
        let full = tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
        assert_eq!(full["servers"].as_array().unwrap().len(), 3);
        assert_eq!(full["fingerprint"].as_str().unwrap(), fp);
    }

    #[tokio::test]
    async fn list_rejects_stale_cursor() {
        let tool = UseCapabilityTool::new(three_server_pool());
        let ctx = test_ctx();
        let err = tool
            .execute(
                json!({"action": "list", "cursor": "deadbeefdeadbeef:0"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("expired"));
        assert!(!err.retryable);
        let bad = tool
            .execute(json!({"action": "list", "cursor": "not-a-cursor"}), &ctx)
            .await
            .unwrap_err();
        assert!(bad.message.contains("invalid cursor"));
    }
}
