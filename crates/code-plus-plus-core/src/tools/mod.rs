pub mod agent_pty;
pub mod builtin;
pub mod display;
pub mod list;
pub mod plan_complete;
pub mod question;
pub mod registry;
pub mod services;
pub mod task;
pub mod tool_result_page;
pub mod use_capability;
pub mod websearch;

pub use registry::ToolRegistry;
pub use services::{CancelToken, EventToolProgressSink, ToolProgressSink, ToolServices};

/// Extract file paths from a unified diff `patch` string for policy checks.
/// Parses `+++ b/<path>` / `+++ <path>` headers; used by policy pattern
/// extraction so `apply_patch` cannot bypass `outside_workspace` guards.
pub fn parse_patch_paths(patch: &str) -> Vec<String> {
    tracing::debug!(
        target: code_plus_plus_telemetry::TOOL,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        patch_bytes = patch.len(),
        "parse_patch_paths started"
    );
    let mut out = Vec::new();
    for line in patch.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("+++ ") {
            let mut p = rest.trim();
            // Strip git a/ b/ prefixes and trailing timestamps/tabs.
            if let Some(s) = p.strip_prefix("b/") {
                p = s;
            } else if let Some(s) = p.strip_prefix("a/") {
                p = s;
            }
            let p = p.split('\t').next().unwrap_or(p).trim();
            let p = p.trim_matches('"').trim();
            if p.is_empty() || p == "/dev/null" {
                continue;
            }
            out.push(p.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}
