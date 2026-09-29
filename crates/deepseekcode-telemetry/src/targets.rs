//! Canonical tracing target names for DeepSeekCode subsystems.

pub const APP: &str = "deepseekcode::app";
pub const SESSION: &str = "deepseekcode::session";
pub const LLM: &str = "deepseekcode::llm";
pub const TOOL: &str = "deepseekcode::tool";
pub const PERMISSION: &str = "deepseekcode::permission";
pub const STORAGE: &str = "deepseekcode::storage";
pub const SERVER: &str = "deepseekcode::server";
pub const DESKTOP: &str = "deepseekcode::desktop";
pub const CLI: &str = "deepseekcode::cli";
pub const AUDIT: &str = "deepseekcode::audit";

pub fn is_session_target(target: &str) -> bool {
    target.starts_with(SESSION)
}

/// Targets that belong in `session.log` for end-to-end troubleshooting.
/// Session lifecycle alone is insufficient: tool executions and LLM calls
/// carry `session_id` and are required to reconstruct a turn.
pub fn is_session_log_target(target: &str) -> bool {
    target.starts_with(SESSION) || target.starts_with(TOOL) || target.starts_with(LLM)
}

pub fn is_audit_target(target: &str) -> bool {
    target.starts_with(AUDIT) || target.starts_with(PERMISSION)
}

/// All canonical subsystem targets. Used to build the default `EnvFilter`
/// so no subsystem is silently filtered when `RUST_LOG` is unset.
pub fn all_targets() -> &'static [&'static str] {
    &[
        APP, SESSION, LLM, TOOL, PERMISSION, STORAGE, SERVER, DESKTOP, CLI, AUDIT,
    ]
}
