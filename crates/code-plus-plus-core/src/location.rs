use code_plus_plus_config::CodePlusPlusConfig;
use code_plus_plus_plugin::{HookRegistry, PluginHost};
use code_plus_plus_storage::{normalize_directory_path, CredentialsStore, Database, PolicyStore};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
        Arc<tokio::sync::RwLock<std::collections::HashSet<code_plus_plus_types::SessionId>>>,
    wasm_tools: Arc<tokio::sync::RwLock<crate::tools::ToolRegistry>>,
}

pub struct LocationMap {
    locations: RwLock<HashMap<String, Arc<LocationServices>>>,
    db: Arc<Database>,
    config: RwLock<CodePlusPlusConfig>,
    credentials: Arc<CredentialsStore>,
    /// Last successful `get_or_create` per location for idle-TTL eviction.
    /// Dropping a `LocationServices` drops its in-memory runners; the DB
    /// epoch stays in SQLite so eviction is lossless.
    last_access: RwLock<HashMap<String, Instant>>,
}

/// Idle TTL for cached locations (30min). Exceeding entries are evicted on
/// the next `get_or_create` so long-lived processes do not leak runners.
pub const LOCATION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

impl LocationMap {
    pub fn new(db: Arc<Database>, config: CodePlusPlusConfig) -> Self {
        let credentials = Arc::new(CredentialsStore::new(db.pool().clone()));
        Self {
            locations: RwLock::new(HashMap::new()),
            db,
            config: RwLock::new(config),
            credentials,
            last_access: RwLock::new(HashMap::new()),
        }
    }

    async fn touch(&self, project_dir: &str) {
        self.last_access
            .write()
            .await
            .insert(project_dir.to_string(), Instant::now());
    }

    /// Evict locations idle longer than `ttl`. Runner state is in-memory
    /// only; the context epoch stays in SQLite, so eviction never loses
    /// durable state. Called opportunistically from `get_or_create`.
    pub async fn evict_idle_locations(&self, ttl: Duration) {
        let now = Instant::now();
        let idle: Vec<String> = {
            let guard = self.last_access.read().await;
            guard
                .iter()
                .filter(|(_, at)| now.duration_since(**at) > ttl)
                .map(|(k, _)| k.clone())
                .collect()
        };
        if idle.is_empty() {
            return;
        }
        {
            let mut locations = self.locations.write().await;
            for dir in &idle {
                locations.remove(dir);
            }
        }
        {
            let mut guard = self.last_access.write().await;
            for dir in &idle {
                guard.remove(dir);
            }
        }
        tracing::info!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            evicted = idle.len(),
            "location idle eviction completed"
        );
    }

    pub async fn update_config(&self, config: CodePlusPlusConfig) {
        tracing::info!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "location update_config started"
        );
        // Unified lock order: locations -> config. Each lock is held short
        // and never nested (no `config.write` held across a `locations`
        // acquisition) so `update_config` cannot ABBA-deadlock with
        // `get_or_create` (which is locations.write -> config.read).
        // Per-location config: previously a single global config was shared
        // and providers/mcp/shell/plugins/session_service never refreshed.
        // Now reload policy (with DB grants) + providers + materialization.
        let project_dirs: Vec<String> = {
            let locations = self.locations.read().await;
            locations.keys().cloned().collect()
        };
        {
            // Short write, dropped before any `locations` acquisition below.
            *self.config.write().await = config.clone();
        }
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
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_dir = %project_dir,
            "location get_or_create started"
        );
        let project_dir = normalize_directory_path(project_dir);
        // Fast-path: short read, never hold `write` across awaits (P0: previously
        // `locations.write()` was held across spawn_blocking/providers/mcp/plugin
        // which blocked all locations on one slow location).
        if let Some(services) = self.locations.read().await.get(&project_dir).cloned() {
            self.touch(&project_dir).await;
            return services;
        }
        // Opportunistic idle eviction (lossless: DB epoch stays in SQLite).
        self.evict_idle_locations(LOCATION_IDLE_TTL).await;

        let config = self.config.read().await.clone();
        let policy_store = PolicyStore::new(self.db.pool().clone());
        let events = Arc::new(code_plus_plus_storage::EventStore::new(
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
                target: code_plus_plus_telemetry::APP,
                run_id = %code_plus_plus_telemetry::current_run_id(),
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
        let trust = code_plus_plus_plugin::verify::PluginTrust {
            require_signatures: config.plugins_require_signatures,
            trusted_keys: config.plugins_trusted_keys.clone(),
        };
        if let Err(e) =
            plugin_host.load_from_config_with_trust(&config.plugins, &project_dir, &trust)
        {
            tracing::warn!(
                target: code_plus_plus_telemetry::APP,
                run_id = %code_plus_plus_telemetry::current_run_id(),
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
        code_plus_plus_plugin::set_wasm_tool_proxy(Arc::new(move |tool, args| {
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
                        let message_id = code_plus_plus_types::MessageId::new();
                        let call_id = code_plus_plus_types::ToolCallId::new();
                        registry
                            .execute(
                                &tool, args_value, session_id, message_id, call_id, "build", None,
                                None,
                            )
                            .await
                    }),
                    Err(e) => Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
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
        // Insert-if-absent: another task may have created it while we built
        // outside the lock. Never overwrite the winner.
        {
            let mut locations = self.locations.write().await;
            if let Some(existing) = locations.get(&project_dir) {
                return existing.clone();
            }
            locations.insert(project_dir.clone(), services.clone());
        }
        self.touch(&project_dir).await;
        services
    }

    /// Interrupt all draining sessions across locations (embedded shutdown).
    pub async fn interrupt_all(&self) {
        tracing::info!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
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
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
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
