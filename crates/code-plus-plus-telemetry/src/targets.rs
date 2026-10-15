//! Canonical tracing target names for Code++ subsystems.

pub const APP: &str = "code-plus-plus::app";
pub const SESSION: &str = "code-plus-plus::session";
pub const LLM: &str = "code-plus-plus::llm";
pub const TOOL: &str = "code-plus-plus::tool";
pub const PERMISSION: &str = "code-plus-plus::permission";
pub const STORAGE: &str = "code-plus-plus::storage";
pub const SERVER: &str = "code-plus-plus::server";
pub const DESKTOP: &str = "code-plus-plus::desktop";
pub const CLI: &str = "code-plus-plus::cli";
pub const AUDIT: &str = "code-plus-plus::audit";

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
