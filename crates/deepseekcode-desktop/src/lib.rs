#![recursion_limit = "256"]

mod activity_leaf;
mod activity_rows;
mod activity_timeline;
mod detail_inspector;
mod git_panel;
mod todo_panel;
mod panel_resize;
pub mod logic;
#[cfg(test)]
mod acceptance_checklist;
mod app;
mod assets;
mod composer;
mod context_usage;
mod home;
mod layout;
mod markdown;
mod editor;
mod file_editor_input;
mod file_tree;
mod multi_window;
mod permission_dock;
mod plan_implement_dock;
mod plan_session_strip;
mod revert;
mod right_panel;
mod session;
mod session_timeline_view;
mod session_activity;
mod session_header;
mod sidebar;
mod syntax;
mod preferences;
mod settings;
mod terminal;
mod terminal_vt;
mod theme;
mod titlebar;
mod ui;
mod workspace;

use app::DeepSeekCodeApp;
use deepseekcode_config::LoggingConfig;

pub fn start() {
    let logging = load_logging_config();
    let run_ctx = deepseekcode_telemetry::init("desktop", &logging);
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %run_ctx.run_id,
        logs_dir = %run_ctx.run_dir.display(),
        "deepseekcode-desktop starting"
    );

    DeepSeekCodeApp::run();
}

fn load_logging_config() -> LoggingConfig {
    deepseekcode_config::load_effective_config(camino::Utf8Path::new("."))
        .map(|c| c.logging)
        .unwrap_or_default()
}
