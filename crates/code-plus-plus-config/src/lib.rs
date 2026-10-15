pub mod loader;
pub mod policy;
pub mod schema;

pub use loader::{discover_config_files, load_effective_config, ConfigPaths};
pub use policy::{
    lines_to_policy, merge_rulesets, policy_to_lines, ruleset_from_config,
    ruleset_from_permissions, ruleset_permission, PolicyConfig, PolicyScopeConfig,
};
pub use schema::CodePlusPlusConfig;
pub use schema::{
    resolve_language, CompactionConfig, CompactionKeepConfig, LogFormat, LoggingConfig,
    PermissionsConfig, PolicyMode, ProviderConfig, ToolOutputConfig, UiConfig,
};
