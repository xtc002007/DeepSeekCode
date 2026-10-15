use crate::policy::{ruleset_permission, PolicyConfig, PolicyScopeConfig};
use crate::schema::{CodePlusPlusConfig, PolicyMode};
use camino::{Utf8Path, Utf8PathBuf};
use code_plus_plus_schema::policy::PolicyRule;
use code_plus_plus_types::{CodePlusPlusError, Result, CONFIG_FILE, LOCAL_DIR};
use std::collections::HashMap;
use std::fs;

// Single source of truth: file/dir names derive from branding constants so a
// rename cannot silently drop config discovery (previously hardcoded in four
// places below).
fn config_names() -> Vec<String> {
    let mut names = vec![CONFIG_FILE.to_string()];
    if let Some(json) = CONFIG_FILE.strip_suffix("jsonc") {
        names.push(format!("{json}json"));
    }
    names
}

fn config_stems() -> Vec<String> {
    fn stem(name: &str) -> String {
        name.split('.').next().unwrap_or(name).to_string()
    }
    vec![stem(CONFIG_FILE)]
}

fn config_subdirs() -> Vec<String> {
    vec![LOCAL_DIR.to_string()]
}

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub global_dir: Utf8PathBuf,
    pub project_dir: Utf8PathBuf,
    pub local_dir: Utf8PathBuf,
}

pub fn discover_config_files(project_dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();

    if let Some(global) =
        code_plus_plus_types::config_dir().and_then(|p| Utf8PathBuf::try_from(p).ok())
    {
        for global_name in config_stems() {
            let global_root = global.join(&global_name);
            for name in config_names() {
                let path = global_root.join(&name);
                if path.exists() {
                    files.push(path);
                }
            }
        }
    }

    // Capture the global-file count before walking so only the hierarchy walk
    // below is reversed: merged config applies root-first and the project file
    // last, so the closest directory wins.
    let global_len = files.len();

    let mut current = Some(project_dir.to_path_buf());
    while let Some(dir) = current {
        for name in config_names() {
            let path = dir.join(&name);
            if path.exists() {
                files.push(path);
            }
        }
        for subdir in config_subdirs() {
            for name in config_names() {
                let path = dir.join(&subdir).join(&name);
                if path.exists() {
                    files.push(path);
                }
            }
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }

    files[global_len..].reverse();

    files
}

pub fn load_effective_config(project_dir: &Utf8Path) -> Result<CodePlusPlusConfig> {
    let files = discover_config_files(project_dir);
    let mut merged = CodePlusPlusConfig::default();

    for file in files {
        let content = fs::read_to_string(file.as_std_path())
            .map_err(|e| CodePlusPlusError::Config(format!("read {}: {e}", file)))?;
        // Accept comments in both .json and .jsonc (case-insensitive):
        // try JSONC first, fall back to strict JSON.
        let ext = file.extension().unwrap_or_default().to_ascii_lowercase();
        let value: serde_json::Value = if ext == "jsonc" || ext == "json" {
            match parse_jsonc(&content) {
                Ok(v) => v,
                Err(_) => serde_json::from_str(&content)?,
            }
        } else {
            serde_json::from_str(&content)?
        };
        merge_config(&mut merged, value)?;
    }

    finalize_policy(&mut merged);
    apply_policy_mode_override(&mut merged);
    Ok(merged)
}

/// Runtime override via `CODE_PLUS_PLUS_POLICY_MODE=auto|yolo|interactive`.
fn apply_policy_mode_override(config: &mut CodePlusPlusConfig) {
    if let Ok(mode) = std::env::var("CODE_PLUS_PLUS_POLICY_MODE") {
        if let Some(parsed) = PolicyMode::parse(&mode) {
            config.policy_mode = Some(parsed);
        }
    }
}

/// Merge legacy `permissions` allow/ask/deny into `policy` without dropping.
/// Explicit `policy` entries always win: legacy rules are only inserted when
/// the same scope+pattern is absent. This fixes both the old
/// "policy non-empty => legacy silently dropped" and the service-side
/// "legacy overwrites explicit policy" contradictions.
fn finalize_policy(config: &mut CodePlusPlusConfig) {
    if config.permissions.allow.is_empty()
        && config.permissions.ask.is_empty()
        && config.permissions.deny.is_empty()
    {
        return;
    }
    for rule in crate::policy::ruleset_from_permissions(&config.permissions) {
        insert_rule_into_policy_if_absent(&mut config.policy, rule);
    }
}

fn insert_rule_into_policy(policy: &mut PolicyConfig, rule: PolicyRule) {
    insert_rule_into_policy_if_absent(policy, rule);
}

fn insert_rule_into_policy_if_absent(policy: &mut PolicyConfig, rule: PolicyRule) {
    if rule.pattern == "*" {
        // Never clobber an explicit Patterns map with a Simple catch-all.
        match policy.get(&rule.scope) {
            None => {
                policy.insert(rule.scope, PolicyScopeConfig::Simple(rule.action));
            }
            Some(PolicyScopeConfig::Simple(_)) => {
                // Explicit simple already wins; keep it.
            }
            Some(PolicyScopeConfig::Patterns(map)) => {
                if !map.contains_key("*") {
                    let mut map = map.clone();
                    map.insert("*".to_string(), rule.action);
                    policy.insert(rule.scope, PolicyScopeConfig::Patterns(map));
                }
            }
        }
        return;
    }
    match policy.get_mut(&rule.scope) {
        Some(PolicyScopeConfig::Patterns(map)) => {
            map.entry(rule.pattern).or_insert(rule.action);
        }
        Some(PolicyScopeConfig::Simple(existing)) => {
            // Preserve explicit Simple as `*` entry, add legacy pattern alongside.
            let mut map = HashMap::new();
            map.insert("*".to_string(), *existing);
            map.entry(rule.pattern).or_insert(rule.action);
            policy.insert(rule.scope, PolicyScopeConfig::Patterns(map));
        }
        None => {
            let mut map = HashMap::new();
            map.insert(rule.pattern, rule.action);
            policy.insert(rule.scope, PolicyScopeConfig::Patterns(map));
        }
    }
}

fn parse_jsonc(content: &str) -> Result<serde_json::Value> {
    use jsonc_parser::{parse_to_serde_value, ParseOptions};
    parse_to_serde_value(content, &ParseOptions::default())
        .map_err(|e| CodePlusPlusError::Config(format!("jsonc parse: {e:?}")))?
        .ok_or_else(|| CodePlusPlusError::Config("empty jsonc document".into()))
}

fn merge_config(base: &mut CodePlusPlusConfig, overlay: serde_json::Value) -> Result<()> {
    if let Some(permission) = overlay.get("permission") {
        for rule in ruleset_permission(permission) {
            insert_rule_into_policy(&mut base.policy, rule);
        }
    }
    let overlay_config: CodePlusPlusConfig = serde_json::from_value(overlay)?;
    if !overlay_config.agents.is_empty() {
        base.agents.extend(overlay_config.agents);
    }
    base.permissions
        .allow
        .extend(overlay_config.permissions.allow);
    base.permissions.ask.extend(overlay_config.permissions.ask);
    base.permissions
        .deny
        .extend(overlay_config.permissions.deny);
    for (scope, cfg) in overlay_config.policy {
        // Merge per-scope instead of whole-scope overwrite so a child file
        // `Simple("*")` never wipes the parent's full pattern table.
        match (base.policy.get_mut(&scope), cfg) {
            (
                Some(PolicyScopeConfig::Patterns(base_map)),
                PolicyScopeConfig::Patterns(overlay_map),
            ) => {
                for (pat, act) in overlay_map {
                    base_map.insert(pat, act);
                }
            }
            (Some(PolicyScopeConfig::Patterns(base_map)), PolicyScopeConfig::Simple(act)) => {
                base_map.insert("*".to_string(), act);
            }
            (Some(PolicyScopeConfig::Simple(_)), PolicyScopeConfig::Patterns(overlay_map)) => {
                let mut map = HashMap::new();
                if let Some(PolicyScopeConfig::Simple(prev)) = base.policy.get(&scope) {
                    map.insert("*".to_string(), *prev);
                }
                for (pat, act) in overlay_map {
                    map.insert(pat, act);
                }
                base.policy.insert(scope, PolicyScopeConfig::Patterns(map));
            }
            (_, overlay_cfg) => {
                base.policy.insert(scope, overlay_cfg);
            }
        }
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
    let global_dir = code_plus_plus_types::config_dir()
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .map(|p| p.join("code-plus-plus"))
        .unwrap_or_else(|| Utf8PathBuf::from(".code-plus-plus"));
    ConfigPaths {
        global_dir,
        project_dir: project_dir.to_path_buf(),
        local_dir: project_dir.join(".code-plus-plus"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_project_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("code-plus-plus.jsonc");
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

    #[test]
    fn discover_orders_project_config_last() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        let mid = root.join("crates");
        let leaf = mid.join("app");
        for path in [&root, &mid, &leaf] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("code-plus-plus.jsonc"), "{}").unwrap();
        }
        let project = Utf8Path::from_path(&leaf).unwrap();
        let files = discover_config_files(project);
        let names: Vec<String> = files
            .iter()
            .map(|f| f.as_str().replace('\\', "/"))
            .collect();
        assert!(names[0].ends_with("repo/code-plus-plus.jsonc"), "{names:?}");
        assert!(names[1].ends_with("crates/code-plus-plus.jsonc"), "{names:?}");
        assert!(names[2].ends_with("app/code-plus-plus.jsonc"), "{names:?}");
    }

    #[test]
    fn project_config_overrides_parent_config() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        let project = root.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(root.join("code-plus-plus.jsonc"), r#"{"shell": "cmd.exe"}"#).unwrap();
        std::fs::write(
            project.join("code-plus-plus.jsonc"),
            r#"{"shell": "pwsh.exe"}"#,
        )
        .unwrap();
        let project_path = Utf8Path::from_path(&project).unwrap();
        let config = load_effective_config(project_path).unwrap();
        assert_eq!(config.shell.as_deref(), Some("pwsh.exe"));
    }
}
