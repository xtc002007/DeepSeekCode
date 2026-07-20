use crate::policy::{ruleset_permission, PolicyConfig, PolicyScopeConfig};
use crate::schema::{DeepSeekCodeConfig, PolicyMode};
use camino::{Utf8Path, Utf8PathBuf};
use deepseekcode_schema::policy::PolicyRule;
use deepseekcode_types::{DeepSeekCodeError, Result};
use std::collections::HashMap;
use std::fs;

const CONFIG_NAMES: &[&str] = &[
    "deepseekcode.jsonc",
    "deepseekcode.json",
    "codevibe.jsonc",
    "codevibe.json",
];

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub global_dir: Utf8PathBuf,
    pub project_dir: Utf8PathBuf,
    pub local_dir: Utf8PathBuf,
}

pub fn discover_config_files(project_dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    if let Some(global) = dirs::config_dir().and_then(|p| Utf8PathBuf::try_from(p).ok()) {
        for global_name in ["deepseekcode", "codevibe"] {
            let global_root = global.join(global_name);
            for name in CONFIG_NAMES {
                let path = global_root.join(name);
                if path.exists() {
                    files.push(path);
                }
            }
        }
    }

    let mut current = Some(project_dir.to_path_buf());
    while let Some(dir) = current {
        for name in CONFIG_NAMES {
            let path = dir.join(name);
            if path.exists() {
                files.push(path);
            }
        }
        for subdir in [".deepseekcode", ".codevibe"] {
            for name in CONFIG_NAMES {
                let path = dir.join(subdir).join(name);
                if path.exists() {
                    files.push(path);
                }
            }
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }

    files
}

pub fn load_effective_config(project_dir: &Utf8Path) -> Result<DeepSeekCodeConfig> {
    let files = discover_config_files(project_dir);
    let mut merged = DeepSeekCodeConfig::default();

    for file in files {
        let content = fs::read_to_string(file.as_std_path())
            .map_err(|e| DeepSeekCodeError::Config(format!("read {}: {e}", file)))?;
        let value: serde_json::Value = if file.extension() == Some("jsonc") {
            parse_jsonc(&content)?
        } else {
            serde_json::from_str(&content)?
        };
        merge_config(&mut merged, value)?;
    }

    finalize_policy(&mut merged);
    apply_policy_mode_override(&mut merged);
    Ok(merged)
}

/// Runtime override via `DEEPSEEKCODE_POLICY_MODE=auto|yolo|interactive`.
fn apply_policy_mode_override(config: &mut DeepSeekCodeConfig) {
    if let Ok(mode) = std::env::var("DEEPSEEKCODE_POLICY_MODE") {
        if let Some(parsed) = PolicyMode::parse(&mode) {
            config.policy_mode = Some(parsed);
        }
    }
}

/// Apply legacy `permissions` allow/deny when no explicit `policy` entries exist.
fn finalize_policy(config: &mut DeepSeekCodeConfig) {
    if config.policy.is_empty()
        && (!config.permissions.allow.is_empty() || !config.permissions.deny.is_empty())
    {
        for rule in crate::policy::ruleset_from_permissions(&config.permissions) {
            insert_rule_into_policy(&mut config.policy, rule);
        }
    }
}

fn insert_rule_into_policy(policy: &mut PolicyConfig, rule: PolicyRule) {
    if rule.pattern == "*" {
        policy.insert(rule.scope, PolicyScopeConfig::Simple(rule.action));
        return;
    }
    match policy.get_mut(&rule.scope) {
        Some(PolicyScopeConfig::Patterns(map)) => {
            map.insert(rule.pattern, rule.action);
        }
        _ => {
            let mut map = HashMap::new();
            map.insert(rule.pattern, rule.action);
            policy.insert(rule.scope, PolicyScopeConfig::Patterns(map));
        }
    }
}

fn parse_jsonc(content: &str) -> Result<serde_json::Value> {
    use jsonc_parser::{parse_to_serde_value, ParseOptions};
    parse_to_serde_value(content, &ParseOptions::default())
        .map_err(|e| DeepSeekCodeError::Config(format!("jsonc parse: {e:?}")))?
        .ok_or_else(|| DeepSeekCodeError::Config("empty jsonc document".into()))
}

fn merge_config(base: &mut DeepSeekCodeConfig, overlay: serde_json::Value) -> Result<()> {
    if let Some(permission) = overlay.get("permission") {
        for rule in ruleset_permission(permission) {
            insert_rule_into_policy(&mut base.policy, rule);
        }
    }
    let overlay_config: DeepSeekCodeConfig = serde_json::from_value(overlay)?;
    if !overlay_config.agents.is_empty() {
        base.agents.extend(overlay_config.agents);
    }
    base.permissions
        .allow
        .extend(overlay_config.permissions.allow);
    base.permissions
        .deny
        .extend(overlay_config.permissions.deny);
    for (scope, cfg) in overlay_config.policy {
        base.policy.insert(scope, cfg);
    }
    base.providers.extend(overlay_config.providers);
    if overlay_config.model.is_some() {
        base.model = overlay_config.model;
    }
    if overlay_config.mcp.is_some() {
        base.mcp = overlay_config.mcp;
    }
    base.instructions.extend(overlay_config.instructions);
    base.skills.extend(overlay_config.skills);
    if overlay_config.compaction.is_some() {
        base.compaction = overlay_config.compaction;
    }
    if overlay_config.tool_output.is_some() {
        base.tool_output = overlay_config.tool_output;
    }
    if overlay_config.shell.is_some() {
        base.shell = overlay_config.shell;
    }
    base.plugins.extend(overlay_config.plugins);
    if overlay_config.autoupdate.is_some() {
        base.autoupdate = overlay_config.autoupdate;
    }
    if overlay_config.policy_mode.is_some() {
        base.policy_mode = overlay_config.policy_mode;
    }
    Ok(())
}

pub fn config_paths(project_dir: &Utf8Path) -> ConfigPaths {
    let global_dir = dirs::config_dir()
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .map(|p| p.join("deepseekcode"))
        .unwrap_or_else(|| Utf8PathBuf::from(".deepseekcode"));
    ConfigPaths {
        global_dir,
        project_dir: project_dir.to_path_buf(),
        local_dir: project_dir.join(".deepseekcode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_project_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deepseekcode.jsonc");
        std::fs::write(
            &path,
            r#"{"shell": "powershell.exe", "instructions": ["AGENTS.md"]}"#,
        )
        .unwrap();
        let project = Utf8Path::from_path(dir.path()).unwrap();
        let config = load_effective_config(project).unwrap();
        assert_eq!(config.shell.as_deref(), Some("powershell.exe"));
        assert_eq!(config.instructions, vec!["AGENTS.md"]);
    }
}
