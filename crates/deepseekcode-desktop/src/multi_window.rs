//! Multi-window management — tracks spawned GPUI windows per workspace.
//!
//! Each GPUI window owns its own [`WorkspaceView`]. GPUI [`Entity`] sharing across
//! windows is awkward, so all windows share one process-wide
//! [`EmbeddedEngine`] via [`shared_engine`] (see `WorkspaceView::bootstrap_project`).
//! Per-window project directories use separate [`DeepSeekCodeClient`] instances that
//! target the same embedded server with different `x-deepseekcode-directory` headers.

use deepseekcode_sdk::{client::DeepSeekCodeClient, EmbeddedEngine};
use gpui::*;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use tokio::sync::Mutex;

static SHARED_ENGINE: LazyLock<Arc<Mutex<Option<EmbeddedEngine>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

/// Process-wide embedded engine slot shared by every [`WorkspaceView`] window.
pub fn shared_engine() -> Arc<Mutex<Option<EmbeddedEngine>>> {
    Arc::clone(&SHARED_ENGINE)
}

/// Build a directory-scoped client for an already-running embedded engine.
pub fn engine_client_for(
    embedded: &EmbeddedEngine,
    directory: impl Into<String>,
) -> DeepSeekCodeClient {
    let mut client = DeepSeekCodeClient::new(format!("http://{}", embedded.addr), directory.into());
    if let Ok(password) = std::env::var("DEEPSEEKCODE_SERVER_PASSWORD") {
        client = client.with_auth("deepseekcode", password);
    }
    client
}

#[derive(Clone, Debug)]
pub struct WindowState {
    pub id: u64,
    pub project_dir: Option<PathBuf>,
}

pub struct WindowManager {
    windows: Vec<WindowState>,
    next_id: u64,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
        }
    }

    pub fn open(&mut self, project_dir: PathBuf) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            window_id = id,
            path = %project_dir.display(),
            "workspace window registered"
        );
        self.windows.push(WindowState {
            id,
            project_dir: Some(project_dir),
        });
        id
    }

    pub fn list(&self) -> &[WindowState] {
        &self.windows
    }

    pub fn window_summaries(&self) -> Vec<(u64, String)> {
        self.windows
            .iter()
            .map(|w| {
                let path = w
                    .project_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(home)".into());
                (w.id, path)
            })
            .collect()
    }

    pub fn spawn_window(project_dir: PathBuf, cx: &mut App) {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("DeepSeekCode".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let dir = project_dir.clone();
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            path = %project_dir.display(),
            "spawning workspace window"
        );
        if let Err(e) = cx.open_window(options, move |_, cx| {
            cx.new(|cx| crate::workspace::WorkspaceView::with_project(dir, cx))
        }) {
            tracing::error!(
                target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                path = %project_dir.display(),
                error = %e,
                "spawn workspace window failed"
            );
            panic!("open window: {e}");
        }
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}
