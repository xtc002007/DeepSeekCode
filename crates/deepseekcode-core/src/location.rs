use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_plugin::{HookRegistry, PluginHost};
use deepseekcode_storage::{normalize_directory_path, CredentialsStore, Database, PolicyStore};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::mcp::McpConnectionPool;
use crate::permission::PermissionService;
use crate::provider::ProviderCatalog;
use crate::session::SessionService;
use crate::tools::ToolServices;

pub struct LocationServices {
    pub project_dir: String,
    pub session_service: SessionService,
    pub providers: Arc<ProviderCatalog>,
    pub permissions: Arc<PermissionService>,
    pub mcp: Arc<McpConnectionPool>,
    pub hooks: Arc<HookRegistry>,
    pub plugin_host: Arc<RwLock<PluginHost>>,
    pub active_session_ids:
        Arc<tokio::sync::RwLock<std::collections::HashSet<deepseekcode_types::SessionId>>>,
    wasm_tools: Arc<tokio::sync::RwLock<crate::tools::ToolRegistry>>,
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
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "location update_config started"
        );
        // Per-location config: previously a single global config was shared
        // and providers/mcp/shell/plugins/session_service never refreshed.
        // Now reload policy (with DB grants) + providers + materialization.
        let project_dirs: Vec<String> = {
            let locations = self.locations.read().await;
            locations.keys().cloned().collect()
        };
        *self.config.write().await = config.clone();
        for dir in &project_dirs {
            let services_opt = {
                let locations = self.locations.read().await;
                locations.get(dir).cloned()
            };
            if let Some(services) = services_opt {
                let _ = services.permissions.reload_policy(&config).await;
                services.providers.load_from_config(&config.providers).await;
                // Refresh MCP servers + skill materialization per location.
                // Session runners pick up new tool definitions on next turn
                // via reload_materialization below.
            }
        }
        // Materialize for all known locations (not just the caller).
        for dir in &project_dirs {
            self.reload_materialization(dir).await;
        }
    }

    pub async fn get_or_create(&self, project_dir: &str) -> Arc<LocationServices> {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "location get_or_create started"
        );
        let project_dir = normalize_directory_path(project_dir);
        let mut locations = self.locations.write().await;
        if let Some(services) = locations.get(&project_dir) {
            return services.clone();
        }

        let config = self.config.read().await.clone();
        let policy_store = PolicyStore::new(self.db.pool().clone());
        let events = Arc::new(deepseekcode_storage::EventStore::new(
            self.db.pool().clone(),
        ));
        let permissions = Arc::new(
            PermissionService::new(&config, &project_dir, Some(policy_store))
                .with_events(events.clone()),
        );
        let _ = permissions.load_from_db().await;

        let configured_shell = config.shell.clone();
        let shell_env = tokio::task::spawn_blocking(move || {
            crate::shell_env::capture_shell_env(configured_shell.as_deref())
        })
        .await
        .unwrap_or_default();

        let providers = Arc::new(ProviderCatalog::with_store(self.credentials.clone()));
        providers.load_from_config(&config.providers).await;
        let _ = providers.load_from_store().await.map_err(|e| {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
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
        let mcp = Arc::new(McpConnectionPool::new(mcp_servers).with_events(events.clone()));
        mcp.connect_all().await;

        let mut plugin_host = PluginHost::new();
        let trust = deepseekcode_plugin::verify::PluginTrust {
            require_signatures: config.plugins_require_signatures,
            trusted_keys: config.plugins_trusted_keys.clone(),
        };
        if let Err(e) =
            plugin_host.load_from_config_with_trust(&config.plugins, &project_dir, &trust)
        {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                error = %e,
                "failed to load plugins"
            );
        }
        let mut hooks = HookRegistry::new();
        plugin_host.register_hooks(&mut hooks);
        let plugin_host = Arc::new(RwLock::new(plugin_host));
        let hooks = Arc::new(hooks);

        let wasm_tools = Arc::new(tokio::sync::RwLock::new(
            crate::tools::ToolRegistry::with_services_and_shell_env(
                project_dir.clone(),
                &config,
                ToolServices::default(),
                shell_env,
            ),
        ));
        let active_session_ids =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));
        let proxy_tools = wasm_tools.clone();
        deepseekcode_plugin::set_wasm_tool_proxy(Arc::new(move |tool, args| {
            let args_value: serde_json::Value =
                serde_json::from_str(args).map_err(|e| format!("invalid tool args json: {e}"))?;
            // Sync WASM callback: never `block_on` on a current runtime
            // (deadlock/panic). Run the async execute on a dedicated OS
            // thread with its own current-thread runtime, using a blocking
            // read of the registry snapshot (P0-4/P1-9).
            let proxy = proxy_tools.clone();
            let tool = tool.to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let res = match rt {
                    Ok(rt) => rt.block_on(async move {
                        // `blocking_read` avoids holding the async lock across
                        // the thread boundary; registry `execute(&self)` is sync-safe.
                        let registry = proxy.blocking_read();
                        let session_id =
                            crate::tools::services::current_session_id().unwrap_or_default();
                        let message_id = deepseekcode_types::MessageId::new();
                        let call_id = deepseekcode_types::ToolCallId::new();
                        registry
                            .execute(
                                &tool, args_value, session_id, message_id, call_id, "build", None,
                                None,
                            )
                            .await
                    }),
                    Err(e) => Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                        "no tokio runtime for wasm tool proxy: {e}"
                    ))),
                };
                let _ = tx.send(res);
            });
            let result = rx
                .recv()
                .map_err(|e| format!("wasm tool proxy hung up: {e}"))?;
            match result {
                Ok(value) => serde_json::to_string(&value.to_stored()).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }));

        let services = Arc::new(LocationServices {
            project_dir: project_dir.clone(),
            session_service: SessionService::new(
                self.db.clone(),
                config.clone(),
                project_dir.clone(),
                permissions.clone(),
                providers.clone(),
                events,
                mcp.clone(),
                hooks.clone(),
                Some(active_session_ids.clone()),
            ),
            providers,
            permissions,
            mcp,
            hooks,
            plugin_host,
            active_session_ids,
            wasm_tools,
        });
        locations.insert(project_dir, services.clone());
        services
    }

    /// Interrupt all draining sessions across locations (embedded shutdown).
    pub async fn interrupt_all(&self) {
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "location interrupt_all started"
        );
        let locations: Vec<Arc<LocationServices>> = {
            let guard = self.locations.read().await;
            guard.values().cloned().collect()
        };
        for loc in locations {
            if let Ok(active) = loc.session_service.active().await {
                for sid in active {
                    let _ = loc.session_service.interrupt(sid).await;
                }
            }
        }
    }

    /// Re-materialize skill tools and refresh MCP tools on all cached session runners.
    pub async fn reload_materialization(&self, project_dir: &str) {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "location reload_materialization started"
        );
        let project_dir = normalize_directory_path(project_dir);
        let config = self.config.read().await.clone();
        let locations = self.locations.read().await;
        if let Some(services) = locations.get(&project_dir) {
            if let Ok(mut tools) = services.wasm_tools.try_write() {
                tools.refresh_skill_tool(&config);
                tools.set_audit_include_tool_output(config.logging.audit.include_tool_output);
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
