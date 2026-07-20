use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_plugin::{HookRegistry, PluginHost};
use deepseekcode_storage::{CredentialsStore, Database, PolicyStore};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::session::SessionService;

pub struct LocationServices {
    pub project_dir: String,
    pub session_service: SessionService,
    pub providers: Arc<ProviderCatalog>,
    pub permissions: Arc<PermissionService>,
    pub mcp: Arc<McpConnectionPool>,
    pub hooks: Arc<HookRegistry>,
    pub plugin_host: Arc<RwLock<PluginHost>>,
    pub active_session_id: Arc<std::sync::RwLock<Option<deepseekcode_types::SessionId>>>,
    wasm_tools: Arc<std::sync::Mutex<crate::tools::ToolRegistry>>,
}

pub struct LocationMap {
    locations: RwLock<HashMap<String, Arc<LocationServices>>>,
    db: Arc<Database>,
    config: RwLock<DeepSeekCodeConfig>,
    credentials: Arc<CredentialsStore>,
}

impl LocationMap {
    pub fn new(db: Arc<Database>, config: DeepSeekCodeConfig) -> Self {
        let credentials = Arc::new(CredentialsStore::new(db.pool().clone()));
        Self {
            locations: RwLock::new(HashMap::new()),
            db,
            config: RwLock::new(config),
            credentials,
        }
    }

    pub async fn update_config(&self, config: DeepSeekCodeConfig) {
        let project_dirs: Vec<String> = {
            let locations = self.locations.read().await;
            locations.keys().cloned().collect()
        };
        *self.config.write().await = config.clone();
        for dir in project_dirs {
            let locations = self.locations.read().await;
            if let Some(services) = locations.get(&dir) {
                let _ = services.permissions.reload_policy(&config).await;
            }
        }
    }

    pub async fn get_or_create(&self, project_dir: &str) -> Arc<LocationServices> {
        let mut locations = self.locations.write().await;
        if let Some(services) = locations.get(project_dir) {
            return services.clone();
        }

        let config = self.config.read().await.clone();
        let policy_store = PolicyStore::new(self.db.pool().clone());
        let events = Arc::new(deepseekcode_storage::EventStore::new(self.db.pool().clone()));
        let permissions = Arc::new(
            PermissionService::new(&config, project_dir, Some(policy_store)).with_events(events.clone()),
        );
        let _ = permissions.load_from_db().await;

        let providers = Arc::new(ProviderCatalog::with_store(self.credentials.clone()));
        providers
            .load_from_config(&config.providers)
            .await;
        let _ = providers.load_from_store().await.map_err(|e| {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                error = %e,
                "failed to load stored provider credentials"
            );
            e
        });

        let mcp_servers = config
            .mcp
            .as_ref()
            .map(|m| m.servers.clone())
            .unwrap_or_default();
        let mcp = Arc::new(
            McpConnectionPool::new(mcp_servers).with_events(events.clone()),
        );
        mcp.connect_all().await;

        let mut plugin_host = PluginHost::new();
        if let Err(e) = plugin_host.load_from_config(&config.plugins, project_dir) {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                error = %e,
                "failed to load plugins"
            );
        }
        let mut hooks = HookRegistry::new();
        plugin_host.register_hooks(&mut hooks);
        let plugin_host = Arc::new(RwLock::new(plugin_host));
        let hooks = Arc::new(hooks);

        let wasm_tools = Arc::new(std::sync::Mutex::new(crate::tools::ToolRegistry::new(
            project_dir.to_string(),
            &config,
        )));
        let active_session_id = Arc::new(std::sync::RwLock::new(None));
        let proxy_tools = wasm_tools.clone();
        let proxy_active = active_session_id.clone();
        deepseekcode_plugin::set_wasm_tool_proxy(Arc::new(move |tool, args| {
            let args_value: serde_json::Value = serde_json::from_str(args)
                .map_err(|e| format!("invalid tool args json: {e}"))?;
            let registry = proxy_tools
                .lock()
                .map_err(|_| "tool registry lock poisoned".to_string())?;
            let rt = tokio::runtime::Handle::try_current()
                .map_err(|_| "no tokio runtime for wasm tool proxy".to_string())?;
            let session_id = proxy_active
                .read()
                .ok()
                .and_then(|g| *g)
                .unwrap_or_else(deepseekcode_types::SessionId::new);
            let message_id = deepseekcode_types::MessageId::new();
            let call_id = deepseekcode_types::ToolCallId::new();
            let result = rt.block_on(registry.execute(
                tool,
                args_value,
                session_id,
                message_id,
                call_id,
                "build",
                None,
            ));
            match result {
                Ok(value) => serde_json::to_string(&value.to_stored()).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }));

        let services = Arc::new(LocationServices {
            project_dir: project_dir.to_string(),
            session_service: SessionService::new(
                self.db.clone(),
                config.clone(),
                project_dir.to_string(),
                permissions.clone(),
                providers.clone(),
                events,
                mcp.clone(),
                hooks.clone(),
                Some(active_session_id.clone()),
            ),
            providers,
            permissions,
            mcp,
            hooks,
            plugin_host,
            active_session_id,
            wasm_tools,
        });
        locations.insert(project_dir.to_string(), services.clone());
        services
    }

    /// Re-materialize skill tools and refresh MCP tools on all cached session runners.
    pub async fn reload_materialization(&self, project_dir: &str) {
        let config = self.config.read().await.clone();
        let locations = self.locations.read().await;
        if let Some(services) = locations.get(project_dir) {
            if let Ok(mut tools) = services.wasm_tools.lock() {
                tools.refresh_skill_tool(&config);
            }
            services.session_service.reload_materialization().await;
            services.session_service.refresh_mcp_tools().await;
        }
    }
}

/// Empty MCP pool and hook registry for tests and lightweight callers.
pub fn empty_location_deps() -> (Arc<McpConnectionPool>, Arc<HookRegistry>) {
    (
        Arc::new(McpConnectionPool::new(HashMap::new())),
        Arc::new(HookRegistry::new()),
    )
}
