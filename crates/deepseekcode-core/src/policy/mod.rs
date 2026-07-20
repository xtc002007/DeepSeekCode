pub mod defaults;
pub mod evaluate;
pub mod patterns;
pub mod service;

pub use defaults::agent_default_ruleset;
pub use evaluate::{evaluate_patterns, evaluate_rule, merge_rulesets, pattern_matches, scope_matches};
pub use patterns::{
    extract_patterns, outside_workspace_paths, remember_patterns, tool_fingerprint, tool_scope,
};
pub use service::{PermissionService, PolicyOutcome, PolicyService};
