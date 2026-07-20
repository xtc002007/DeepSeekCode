//! Canonical tracing target names for DeepSeekCode subsystems.

pub const APP: &str = "deepseekcode::app";
pub const SESSION: &str = "deepseekcode::session";
pub const LLM: &str = "deepseekcode::llm";
pub const TOOL: &str = "deepseekcode::tool";
pub const PERMISSION: &str = "deepseekcode::permission";
pub const STORAGE: &str = "deepseekcode::storage";
pub const SERVER: &str = "deepseekcode::server";
pub const DESKTOP: &str = "deepseekcode::desktop";
pub const AUDIT: &str = "deepseekcode::audit";

pub fn is_session_target(target: &str) -> bool {
    target.starts_with(SESSION)
}

pub fn is_audit_target(target: &str) -> bool {
    target.starts_with(AUDIT) || target.starts_with(PERMISSION)
}
