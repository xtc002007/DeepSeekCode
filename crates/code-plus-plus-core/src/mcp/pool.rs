use code_plus_plus_config::schema::McpServerConfig;
use code_plus_plus_schema::event::EventKind;
use code_plus_plus_storage::EventStore;
use code_plus_plus_types::{CodePlusPlusError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use super::local_stdio::{LocalMcpClient, McpToolInfo};
use super::remote::RemoteMcpClient;
use super::tool_bridge::{bridged_tool_name, bridged_tools_from_pool};
use crate::tools::registry::Tool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub connected: bool,
    pub tools: Vec<String>,
}

enum McpConnection {
    // Sync mutex: only locked inside `spawn_blocking`, never across `.await`.
    Local(Arc<std::sync::Mutex<LocalMcpClient>>),
    Remote(Arc<RemoteMcpClient>),
}

struct ConnectedServer {
    tools: Vec<McpToolInfo>,
    connection: McpConnection,
}

/// Location-scoped MCP connection pool. One instance per project directory.
pub struct McpConnectionPool {
    servers: HashMap<String, McpServerConfig>,
    connected: Arc<Mutex<HashMap<String, ConnectedServer>>>,
    events: Option<Arc<EventStore>>,
}

impl McpConnectionPool {
    pub fn new(servers: HashMap<String, McpServerConfig>) -> Self {
        Self {
            servers,
            connected: Arc::new(Mutex::new(HashMap::new())),
            events: None,
        }
    }

    pub fn with_events(mut self, events: Arc<EventStore>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn connect_all(&self) {
        // Concurrent with per-server timeout so one slow server cannot block
        // location startup. tokio::Mutex is held only for map insert.
        use futures::stream::{FuturesUnordered, StreamExt};
        let mut pending = FuturesUnordered::new();
        for (name, cfg) in &self.servers {
            if cfg.enabled.unwrap_or(true) {
                let name = name.clone();
                // Note: `connect` borrows self; spawn via timeout wrapper.
                pending.push(async move {
                    let res =
                        tokio::time::timeout(Duration::from_secs(15), self.connect(&name)).await;
                    (name, res)
                });
            }
        }
        while let Some((name, res)) = pending.next().await {
            match res {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(
                    target: code_plus_plus_telemetry::APP,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    server = %name,
                    error = %e,
                    "mcp auto-connect failed"
                ),
                Err(_) => tracing::warn!(
                    target: code_plus_plus_telemetry::APP,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    server = %name,
                    "mcp auto-connect timed out after 15s"
                ),
            }
        }
    }

    pub async fn connect(&self, name: &str) -> Result<Vec<String>> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            server = %name,
            "mcp connect started"
        );
        let cfg = self
            .servers
            .get(name)
            .ok_or_else(|| CodePlusPlusError::NotFound(format!("mcp server {name}")))?
            .clone();

        if !cfg.enabled.unwrap_or(true) {
            return Err(CodePlusPlusError::Other(format!(
                "mcp server {name} is disabled"
            )));
        }

        let (connection, tools) = if let Some(command) = cfg.command.as_deref() {
            let args = cfg.args.clone().unwrap_or_default();
            let command = command.to_string();
            let (connection, tools) = tokio::task::spawn_blocking(move || {
                let mut client = LocalMcpClient::spawn(&command, &args)?;
                client.initialize()?;
                let tools = client.list_tools()?;
                Ok::<_, CodePlusPlusError>((Arc::new(std::sync::Mutex::new(client)), tools))
            })
            .await
            .map_err(|e| {
                CodePlusPlusError::Other(format!("mcp local connect join failed: {e}"))
            })??;
            (McpConnection::Local(connection), tools)
        } else if let Some(url) = cfg.url.as_deref() {
            // Fail fast on non-HTTP(S) URLs (my-notebook `validate_http_url`):
            // otherwise a typo'd `file://` URL surfaces as a confusing
            // reqwest builder error deep inside `initialize`.
            let lower = url.trim_start().to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return Err(CodePlusPlusError::Other(format!(
                    "mcp server {name} has unsupported url scheme (expected http:// or https://)"
                )));
            }
            let client = RemoteMcpClient::new(url);
            client.initialize().await?;
            let tools = client.list_tools().await.unwrap_or_default();
            (McpConnection::Remote(Arc::new(client)), tools)
        } else {
            return Err(CodePlusPlusError::Other(format!(
                "mcp server {name} has no command or url"
            )));
        };

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        self.connected
            .lock()
            .await
            .insert(name.to_string(), ConnectedServer { tools, connection });
        self.emit_status(name, true, &tool_names).await;
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            server = %name,
            tools = tool_names.len(),
            "mcp connect completed"
        );
        Ok(tool_names)
    }

    pub async fn disconnect(&self, name: &str) -> Result<()> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            server = %name,
            "mcp disconnect started"
        );
        if self.connected.lock().await.remove(name).is_none() {
            return Err(CodePlusPlusError::NotFound(format!(
                "mcp server {name} not connected"
            )));
        }
        self.emit_status(name, false, &[]).await;
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            server = %name,
            "mcp disconnect completed"
        );
        Ok(())
    }

    async fn emit_status(&self, name: &str, connected: bool, tools: &[String]) {
        let Some(events) = &self.events else {
            return;
        };
        let _ = events
            .append(
                EventKind::McpStatusChanged,
                None,
                json!({
                    "name": name,
                    "connected": connected,
                    "tools": tools,
                    "tools_list_changed": connected && !tools.is_empty(),
                }),
            )
            .await;
    }

    pub fn status(&self) -> Vec<McpServerStatus> {
        // Sync snapshot for UI polling; try_lock to avoid blocking async.
        let Ok(connected) = self.connected.try_lock() else {
            return self
                .servers
                .keys()
                .map(|name| McpServerStatus {
                    name: name.clone(),
                    connected: false,
                    tools: Vec::new(),
                })
                .collect();
        };
        self.servers
            .iter()
            .map(|(name, cfg)| {
                let tools = connected
                    .get(name)
                    .map(|s| s.tools.iter().map(|t| t.name.clone()).collect())
                    .unwrap_or_default();
                McpServerStatus {
                    name: name.clone(),
                    connected: connected.contains_key(name) && cfg.enabled.unwrap_or(true),
                    tools,
                }
            })
            .collect()
    }

    /// Issue-7: snapshot for the stable `use_capability` proxy so catalog
    /// reads never touch the prefix.
    pub fn connected_tools_snapshot(&self) -> Vec<(String, Vec<McpToolInfo>)> {
        self.connected_tools()
    }

    pub(crate) fn connected_tools(&self) -> Vec<(String, Vec<McpToolInfo>)> {
        let Ok(connected) = self.connected.try_lock() else {
            return Vec::new();
        };
        connected
            .iter()
            .map(|(server, entry)| (server.clone(), entry.tools.clone()))
            .collect()
    }

    pub fn bridged_tools(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        bridged_tools_from_pool(self)
    }

    pub async fn call_tool(&self, server: &str, tool: &str, arguments: Value) -> Result<Value> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            server = %server,
            tool = %tool,
            "mcp call_tool started"
        );
        // Single lock acquisition: clone the needed Arc, then release before await.
        let connection = {
            let guard = self.connected.lock().await;
            let entry = guard.get(server).ok_or_else(|| {
                CodePlusPlusError::NotFound(format!("mcp server {server} not connected"))
            })?;
            match &entry.connection {
                McpConnection::Remote(c) => McpConnection::Remote(c.clone()),
                McpConnection::Local(c) => McpConnection::Local(c.clone()),
            }
        };
        match connection {
            McpConnection::Remote(client) => client.call_tool(tool, arguments).await,
            McpConnection::Local(client) => {
                // 30s wall-clock budget per local tool call (my-notebook
                // `STDIO_TIMEOUT`): `LocalMcpClient::request` only checks its
                // deadline *between* `read_line` calls, so a fully silent
                // server would otherwise block this `spawn_blocking` slot —
                // and the session turn — forever.
                let tool = tool.to_string();
                let join = tokio::task::spawn_blocking(move || {
                    let mut locked = client
                        .lock()
                        .map_err(|_| CodePlusPlusError::Other("mcp client lock poisoned".into()))?;
                    locked.call_tool(&tool, arguments)
                });
                match tokio::time::timeout(Duration::from_secs(30), join).await {
                    Ok(joined) => joined.map_err(|e| {
                        CodePlusPlusError::Other(format!("mcp local call join failed: {e}"))
                    })?,
                    Err(_) => Err(CodePlusPlusError::Other(format!(
                        "no MCP response from {server} within 30s"
                    ))),
                }
            }
        }
    }

    pub fn list_registered_tool_names(&self) -> Vec<String> {
        let Ok(connected) = self.connected.try_lock() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for (server, entry) in connected.iter() {
            for tool in &entry.tools {
                names.push(bridged_tool_name(server, &tool.name));
            }
        }
        names
    }
}
