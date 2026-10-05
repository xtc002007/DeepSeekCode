use crate::client::DeepSeekCodeClient;
use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_embed::{build_router, pty::PtyManager, state::AppState};
use deepseekcode_types::Result;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

static DEDICATED_ENGINE_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static USE_DEDICATED_ENGINE_RUNTIME: AtomicBool = AtomicBool::new(false);

/// Desktop clients should call this once at startup so session/tool I/O never shares the UI tokio pool.
pub fn install_dedicated_engine_runtime() {
    match Runtime::new() {
        Ok(rt) => {
            let _ = DEDICATED_ENGINE_RUNTIME.set(rt);
            USE_DEDICATED_ENGINE_RUNTIME.store(true, Ordering::SeqCst);
        }
        Err(e) => {
            tracing::error!(
                target: deepseekcode_telemetry::APP,
                error = %e,
                "dedicated engine runtime creation failed; falling back to current runtime"
            );
        }
    }
}

fn dedicated_engine_handle() -> Handle {
    if let Some(rt) = DEDICATED_ENGINE_RUNTIME.get() {
        return rt.handle().clone();
    }
    // Fallback instead of panicking: embedded can run on the current runtime
    // (with reduced isolation) rather than crashing the host process.
    match Handle::try_current() {
        Ok(h) => {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                "dedicated engine runtime not installed; using current runtime"
            );
            h
        }
        Err(_) => {
            tracing::error!(
                target: deepseekcode_telemetry::APP,
                "no tokio runtime available for embedded engine; creating ephemeral one"
            );
            // Last resort: leak a runtime handle so embedded still works in
            // non-async contexts instead of panicking.
            let rt = Runtime::new().unwrap_or_else(|e| {
                eprintln!("DeepSeekCode embedded runtime unavailable: {e}");
                std::process::exit(1);
            });
            let handle = rt.handle().clone();
            std::mem::forget(rt);
            handle
        }
    }
}

pub struct EmbeddedEngine {
    pub addr: SocketAddr,
    pub client: DeepSeekCodeClient,
    handle: JoinHandle<()>,
    engine: Arc<RwLock<DeepSeekCodeEngine>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl EmbeddedEngine {
    pub async fn start(project_dir: impl Into<String>) -> Result<Self> {
        let project_dir = project_dir.into();
        if USE_DEDICATED_ENGINE_RUNTIME.load(Ordering::SeqCst) {
            let handle = dedicated_engine_handle();
            return handle
                .spawn(async move { Self::start_inner(project_dir).await })
                .await
                .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        }
        Self::start_inner(project_dir).await
    }

    async fn start_inner(project_dir: String) -> Result<Self> {
        // Product-grade fix: embedded engines are started from Desktop, tests,
        // and library users that never go through CLI `init("cli")`. Without
        // this, all `tracing!` below hit the noop subscriber and are lost.
        let run_ctx =
            deepseekcode_telemetry::ensure_init("sdk-embedded", Some(project_dir.as_str()));
        let run_id = run_ctx.run_id.clone();
        let db_path = std::env::var("DEEPSEEKCODE_DB_PATH").unwrap_or_else(|_| {
            deepseekcode_types::data_dir()
                .map(|p| p.join("deepseekcode").join("deepseekcode.db"))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "deepseekcode.db".into())
        });

        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %run_id,
            project_dir = %project_dir,
            db_path = %db_path,
            "starting embedded engine"
        );

        let db_url = format!("sqlite://{db_path}");
        let engine = match DeepSeekCodeEngine::open(project_dir.clone(), &db_url).await {
            Ok(e) => Arc::new(RwLock::new(e)),
            Err(e) => {
                tracing::error!(
                    target: deepseekcode_telemetry::APP,
                    run_id = %run_id,
                    project_dir = %project_dir,
                    db_path = %db_path,
                    error = %e,
                    "embedded engine open failed"
                );
                return Err(e);
            }
        };

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(
                    target: deepseekcode_telemetry::SERVER,
                    run_id = %run_id,
                    error = %e,
                    "embedded engine bind failed"
                );
                return Err(deepseekcode_types::DeepSeekCodeError::Other(e.to_string()));
            }
        };
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(
                    target: deepseekcode_telemetry::SERVER,
                    run_id = %run_id,
                    error = %e,
                    "embedded engine local_addr failed"
                );
                return Err(deepseekcode_types::DeepSeekCodeError::Other(e.to_string()));
            }
        };

        let password = std::env::var("DEEPSEEKCODE_SERVER_PASSWORD").ok();
        let auth_password = password.clone();
        let state = AppState {
            engine: engine.clone(),
            password,
            pty: Arc::new(PtyManager::new()),
            locale: deepseekcode_config::load_effective_config(camino::Utf8Path::new(&project_dir))
                .map(|config| deepseekcode_config::resolve_language(&config))
                .unwrap_or_default(),
        };
        let app = build_router(state);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        // Long-lived serve task: no originating HTTP request exists at startup,
        // so `request_id` stays "-"; `run_id` + `%addr` carry the correlation.
        // `run_id` is captured here (not read inside) because the string is
        // cheap to move and keeps the log stable even across thread hops.
        let serve_run_id = run_id.clone();
        let serve_addr = addr;
        let handle = tokio::spawn(async move {
            let shutdown = async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
            };
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
            {
                tracing::error!(
                    target: deepseekcode_telemetry::SERVER,
                    run_id = %serve_run_id,
                    addr = %serve_addr,
                    error = %e,
                    "embedded axum serve exited"
                );
            }
        });

        let client = if let Some(ref password) = auth_password {
            DeepSeekCodeClient::new(format!("http://{addr}"), project_dir.clone())
                .with_auth("deepseekcode", password.clone())
        } else {
            DeepSeekCodeClient::new(format!("http://{addr}"), project_dir.clone())
        };
        // Axum may not be listening yet: retry health briefly instead of
        // single-shot flaky `connection refused`.
        {
            let mut last_err = String::new();
            let mut ok = false;
            for _ in 0..20 {
                match client.health().await {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            if !ok {
                tracing::error!(
                    target: deepseekcode_telemetry::SERVER,
                    run_id = %run_id,
                    %addr,
                    error = %last_err,
                    "embedded engine health check failed"
                );
                return Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                    "embedded engine health check failed: {last_err}"
                )));
            }
        }

        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %run_id,
            %addr,
            project_dir = %project_dir,
            "embedded engine ready"
        );

        Ok(Self {
            addr,
            client,
            handle,
            engine,
            shutdown_tx,
        })
    }

    pub fn client(&self) -> DeepSeekCodeClient {
        self.client.clone()
    }

    pub fn engine(&self) -> Arc<RwLock<DeepSeekCodeEngine>> {
        self.engine.clone()
    }

    pub async fn shutdown(self) {
        tracing::info!(target: deepseekcode_telemetry::APP, "embedded engine shutting down");
        // Interrupt live drains so no turn is left half-written in SQLite.
        {
            let engine = self.engine.read().await;
            engine.interrupt_all().await;
        }
        let _ = self.shutdown_tx.send(true);
        let handle = self.handle;
        let abort = handle.abort_handle();
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::APP,
                    error = %e,
                    "embedded engine task join error"
                );
            }
            Err(_) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::APP,
                    "embedded engine graceful shutdown timed out; aborting"
                );
                abort.abort();
            }
        }
    }
}
