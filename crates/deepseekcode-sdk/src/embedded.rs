use crate::client::DeepSeekCodeClient;
use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_server::{build_router, pty::PtyManager, state::AppState};
use deepseekcode_types::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

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
        let db_path = std::env::var("DEEPSEEKCODE_DB_PATH").unwrap_or_else(|_| {
            dirs::data_dir()
                .map(|p| p.join("deepseekcode").join("deepseekcode.db"))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "deepseekcode.db".into())
        });

        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        tracing::info!(
            target: deepseekcode_telemetry::APP,
            project_dir = %project_dir,
            db_path = %db_path,
            "starting embedded engine"
        );

        let db_url = format!("sqlite://{db_path}");
        let engine = Arc::new(RwLock::new(
            DeepSeekCodeEngine::open(project_dir.clone(), &db_url).await?,
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        let password = std::env::var("DEEPSEEKCODE_SERVER_PASSWORD").ok();
        let auth_password = password.clone();
        let state = AppState {
            engine: engine.clone(),
            password,
            pty: Arc::new(PtyManager::new()),
        };
        let app = build_router(state);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            let shutdown = async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
            };
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
                tracing::error!(
                    target: deepseekcode_telemetry::SERVER,
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
        client.health().await?;

        tracing::info!(
            target: deepseekcode_telemetry::APP,
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
