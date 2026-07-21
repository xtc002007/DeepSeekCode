use deepseekcode_config::schema::McpServerConfig;
use deepseekcode_schema::event::EventKind;
use deepseekcode_storage::EventStore;
use deepseekcode_types::{DeepSeekCodeError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    Local(LocalMcpClient),
    Remote(Arc<RemoteMcpClient>),
}

struct ConnectedServer {
    tools: Vec<McpToolInfo>,
    connection: McpConnection,
}

/// Location-scoped MCP connection pool. One instance per project directory.
pub struct McpConnectionPool {
    servers: HashMap<String, McpServerConfig>,
    connected: Mutex<HashMap<String, ConnectedServer>>,
    events: Option<Arc<EventStore>>,
}

impl McpConnectionPool {
    pub fn new(servers: HashMap<String, McpServerConfig>) -> Self {
        Self {
            servers,
            connected: Mutex::new(HashMap::new()),
            events: None,
        }
    }

    pub fn with_events(mut self, events: Arc<EventStore>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn connect_all(&self) {
        for (name, cfg) in &self.servers {
            if cfg.enabled.unwrap_or(true) {
                if let Err(e) = self.connect(name).await {
                    tracing::warn!(
                        target: deepseekcode_telemetry::APP,
                        server = %name,
                        error = %e,
                        "mcp auto-connect failed"
                    );
                }
            }
        }
    }

    pub async fn connect(&self, name: &str) -> Result<Vec<String>> {
        let cfg = self
            .servers
            .get(name)
            .ok_or_else(|| DeepSeekCodeError::NotFound(format!("mcp server {name}")))?
            .clone();

        if !cfg.enabled.unwrap_or(true) {
            return Err(DeepSeekCodeError::Other(format!(
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
                Ok::<_, DeepSeekCodeError>((McpConnection::Local(client), tools))
            })
            .await
            .map_err(|e| {
                DeepSeekCodeError::Other(format!("mcp local connect join failed: {e}"))
            })??;
            (connection, tools)
        } else if let Some(url) = cfg.url.as_deref() {
            let client = RemoteMcpClient::new(url);
            client.initialize().await?;
            let tools = client.list_tools().await.unwrap_or_default();
            (McpConnection::Remote(Arc::new(client)), tools)
        } else {
            return Err(DeepSeekCodeError::Other(format!(
                "mcp server {name} has no command or url"
            )));
        };

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        self.connected
            .lock()
            .unwrap()
            .insert(name.to_string(), ConnectedServer { tools, connection });
        self.emit_status(name, true, &tool_names).await;
        Ok(tool_names)
    }

    pub async fn disconnect(&self, name: &str) -> Result<()> {
        if self.connected.lock().unwrap().remove(name).is_none() {
            return Err(DeepSeekCodeError::NotFound(format!(
                "mcp server {name} not connected"
            )));
        }
        self.emit_status(name, false, &[]).await;
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
        let connected = self.connected.lock().unwrap();
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

    pub(crate) fn connected_tools(&self) -> Vec<(String, Vec<McpToolInfo>)> {
        let connected = self.connected.lock().unwrap();
        connected
            .iter()
            .map(|(server, entry)| (server.clone(), entry.tools.clone()))
            .collect()
    }

    pub fn bridged_tools(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        bridged_tools_from_pool(self)
    }

    pub async fn call_tool(&self, server: &str, tool: &str, arguments: Value) -> Result<Value> {
        let remote_client = {
            let mut connected = self.connected.lock().unwrap();
            let entry = connected.get_mut(server).ok_or_else(|| {
                DeepSeekCodeError::NotFound(format!("mcp server {server} not connected"))
            })?;
            match &mut entry.connection {
                McpConnection::Local(client) => {
                    return client.call_tool(tool, arguments);
                }
                McpConnection::Remote(client) => client.clone(),
            }
        };
        remote_client.call_tool(tool, arguments).await
    }

    pub fn list_registered_tool_names(&self) -> Vec<String> {
        let connected = self.connected.lock().unwrap();
        let mut names = Vec::new();
        for (server, entry) in connected.iter() {
            for tool in &entry.tools {
                names.push(bridged_tool_name(server, &tool.name));
            }
        }
        names
    }
}
