#![recursion_limit = "256"]

#[cfg(test)]
mod acceptance_checklist;
mod activity_leaf;
mod activity_rows;
mod activity_timeline;
mod app;
mod assets;
mod composer;
mod context_usage;
mod detail_inspector;
mod editor;
mod file_editor_input;
mod file_tree;
mod git_panel;
mod home;
mod layout;
pub mod logic;
mod markdown;
mod multi_window;
mod panel_resize;
mod permission_dock;
mod plan_implement_dock;
mod plan_session_strip;
mod preferences;
mod revert;
mod right_panel;
pub(crate) mod selectable_text;
mod session;
mod session_activity;
mod session_header;
mod session_timeline_view;
mod settings;
mod sidebar;
mod status_text;
mod syntax;
mod terminal;
pub mod terminal_vscode;
mod terminal_vt;
mod theme;
mod titlebar;
mod todo_panel;
mod ui;
mod workspace;

use app::CodePlusPlusApp;
use code_plus_plus_config::LoggingConfig;

pub fn start() {
    // Agent commands run in REAL interactive PTYs (the terminal tab IS the
    // execution site: same input/output behavior as a hand-opened shell,
    // auto-destroyed on completion). The core gate reads this before any
    // engine boots; CLI/server/tests never set it and keep pipes.
    std::env::set_var("CODE_PLUS_PLUS_BASH_PTY", "1");
    code_plus_plus_sdk::install_dedicated_engine_runtime();
    let (logging, config_error, config_language) = load_logging_config();
    // Language first, before any UI string is rendered: project config
    // `ui.language` wins when pinned, otherwise auto-detect. The workspace
    // later applies the user's Settings choice on top (see `WorkspaceView`).
    code_plus_plus_i18n::init_from_preference(config_language);
    let run_ctx = code_plus_plus_telemetry::init("desktop", &logging);
    tracing::info!(
        target: code_plus_plus_telemetry::APP,
        run_id = %run_ctx.run_id,
        logs_dir = %run_ctx.run_dir.display(),
        "code-plus-plus-desktop starting"
    );
    if let Some(err) = config_error.as_deref() {
        tracing::warn!(
            target: code_plus_plus_telemetry::DESKTOP,
            run_id = %run_ctx.run_id,
            error = %err,
            "config load failed; using default logging"
        );
    }

    CodePlusPlusApp::run();
}

fn load_logging_config() -> (
    LoggingConfig,
    Option<String>,
    code_plus_plus_i18n::LanguagePref,
) {
    match code_plus_plus_config::load_effective_config(camino::Utf8Path::new(".")) {
        Ok(c) => {
            let language = code_plus_plus_config::resolve_language(&c);
            (c.logging, None, language)
        }
        Err(e) => {
            let msg = code_plus_plus_i18n::tr!(
                "cli.err.config_load_failed",
                "directory" => ".",
                "error" => e.to_string()
            );
            eprintln!("code-plus-plus: {msg}");
            (
                Default::default(),
                Some(msg),
                code_plus_plus_i18n::LanguagePref::Auto,
            )
        }
    }
}
