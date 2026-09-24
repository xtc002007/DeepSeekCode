use crate::schema::PermissionsConfig;
use deepseekcode_schema::policy::{PolicyAction, PolicyRule, PolicyRuleset};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Per-scope policy: shorthand action or pattern map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PolicyScopeConfig {
    Simple(PolicyAction),
    Patterns(HashMap<String, PolicyAction>),
}

pub type PolicyConfig = HashMap<String, PolicyScopeConfig>;

pub fn deserialize_policy_action<'de, D>(deserializer: D) -> Result<PolicyAction, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    PolicyAction::parse(&s)
        .ok_or_else(|| serde::de::Error::custom(format!("unknown policy action: {s}")))
}

/// Convert a `PolicyConfig` map into a flat ordered ruleset.
pub fn ruleset_from_config(config: &PolicyConfig) -> PolicyRuleset {
    let mut rules = Vec::new();
    for (scope, value) in config {
        match value {
            PolicyScopeConfig::Simple(action) => {
                rules.push(PolicyRule {
                    scope: scope.clone(),
                    pattern: "*".into(),
                    action: *action,
                });
            }
            PolicyScopeConfig::Patterns(map) => {
                for (pattern, action) in map {
                    rules.push(PolicyRule {
                        scope: scope.clone(),
                        pattern: expand_home(pattern),
                        action: *action,
                    });
                }
            }
        }
    }
    rules
}

/// Merge multiple rulesets; later rulesets append (last-match-wins during evaluation).
pub fn merge_rulesets(sets: impl IntoIterator<Item = PolicyRuleset>) -> PolicyRuleset {
    sets.into_iter().flatten().collect()
}

/// Legacy `permissions.allow/ask/deny` → policy rules.
/// Each entry is split into (scope, pattern) so `git *` becomes
/// `scope=bash pattern=git *` instead of a dead `scope="git *"`.
/// Order: allow, ask, deny — deny last so an explicit block wins ties
/// when the same pattern appears in both lists.
pub fn ruleset_from_permissions(permissions: &PermissionsConfig) -> PolicyRuleset {
    let mut rules = Vec::new();
    for pattern in &permissions.allow {
        let (scope, pat) = legacy_scope_pattern(pattern);
        rules.push(PolicyRule {
            scope,
            pattern: pat,
            action: PolicyAction::Permit,
        });
    }
    for pattern in &permissions.ask {
        let (scope, pat) = legacy_scope_pattern(pattern);
        rules.push(PolicyRule {
            scope,
            pattern: pat,
            action: PolicyAction::Prompt,
        });
    }
    for pattern in &permissions.deny {
        let (scope, pat) = legacy_scope_pattern(pattern);
        rules.push(PolicyRule {
            scope,
            pattern: pat,
            action: PolicyAction::Block,
        });
    }
    rules
}

pub fn ruleset_permission(value: &serde_json::Value) -> PolicyRuleset {
    let mut rules = Vec::new();
    let Some(obj) = value.as_object() else {
        if let Some(s) = value.as_str() {
            if let Some(action) = PolicyAction::parse(s) {
                rules.push(PolicyRule {
                    scope: "*".into(),
                    pattern: "*".into(),
                    action,
                });
            }
        }
        return rules;
    };

    for (scope, cfg) in obj {
        if let Some(s) = cfg.as_str() {
            if let Some(action) = PolicyAction::parse(s) {
                rules.push(PolicyRule {
                    scope: scope.clone(),
                    pattern: "*".into(),
                    action,
                });
            }
            continue;
        }
        if let Some(map) = cfg.as_object() {
            for (pattern, action_val) in map {
                if let Some(s) = action_val.as_str() {
                    if let Some(action) = PolicyAction::parse(s) {
                        rules.push(PolicyRule {
                            scope: scope.clone(),
                            pattern: expand_home(pattern),
                            action,
                        });
                    }
                }
            }
        }
    }
    rules
}

fn legacy_scope_pattern(raw: &str) -> (String, String) {
    let pattern = raw.trim();
    if pattern.is_empty() || pattern == "*" {
        return ("*".into(), "*".into());
    }
    if pattern.starts_with("mcp:") {
        // e.g. `mcp:server:*` stays a scope; pattern is wildcard.
        return (pattern.to_string(), "*".into());
    }
    // Bash-like command prefixes: `git *`, `rm *`, `npm run *`.
    let first = pattern.split_whitespace().next().unwrap_or("");
    const BASH_CMDS: &[&str] = &[
        "git", "rm", "npm", "npx", "cargo", "pnpm", "yarn", "go", "python", "pip", "docker",
        "kubectl", "ls", "cat", "echo", "cd", "mkdir", "rmdir", "cp", "mv", "chmod", "chown",
    ];
    if BASH_CMDS.contains(&first) || pattern.contains('*') && first.len() <= 12 {
        // Heuristic: command-like entries map to bash scope preserving granularity.
        // `*` alone already handled; `*.env` is a read-scope file glob.
        if first.starts_with('*') || first.starts_with('.') {
            return ("read".into(), pattern.to_string());
        }
        return ("bash".into(), pattern.to_string());
    }
    // File globs (`*.env`, `src/**`) map to read scope.
    if pattern.contains('/') || pattern.contains('.') || pattern.contains('*') {
        return ("read".into(), pattern.to_string());
    }
    // Fallback: treat as bash command pattern (never a dead scope).
    ("bash".into(), pattern.to_string())
}

#[allow(dead_code)]
fn legacy_pattern_to_scope(pattern: &str) -> String {
    legacy_scope_pattern(pattern).0
}

fn expand_home(pattern: &str) -> String {
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    pattern.to_string()
}

/// Serialize policy rules as `scope | pattern | action` lines for editors.
pub fn policy_to_lines(config: &PolicyConfig) -> String {
    ruleset_from_config(config)
        .into_iter()
        .map(|rule| {
            let action = match rule.action {
                PolicyAction::Permit => "permit",
                PolicyAction::Prompt => "prompt",
                PolicyAction::Block => "block",
            };
            format!("{} | {} | {}", rule.scope, rule.pattern, action)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse `scope | pattern | action` lines into a policy config.
pub fn lines_to_policy(text: &str) -> PolicyConfig {
    let mut policy = PolicyConfig::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.len() != 3 {
            continue;
        }
        let Some(action) = PolicyAction::parse(parts[2]) else {
            continue;
        };
        let scope = parts[0].to_string();
        let pattern = parts[1].to_string();
        if pattern == "*" {
            policy.insert(scope, PolicyScopeConfig::Simple(action));
        } else {
            match policy.get_mut(&scope) {
                Some(PolicyScopeConfig::Patterns(map)) => {
                    map.insert(pattern, action);
                }
                Some(PolicyScopeConfig::Simple(existing)) => {
                    let mut map = HashMap::new();
                    map.insert("*".into(), *existing);
                    map.insert(pattern, action);
                    policy.insert(scope, PolicyScopeConfig::Patterns(map));
                }
                None => {
                    let mut map = HashMap::new();
                    map.insert(pattern, action);
                    policy.insert(scope, PolicyScopeConfig::Patterns(map));
                }
            }
        }
    }
    policy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shorthand_policy() {
        let cfg: PolicyConfig = serde_json::from_value(serde_json::json!({
            "bash": "prompt",
            "read": { "*": "permit", "*.env": "block" }
        }))
        .unwrap();
        let rules = ruleset_from_config(&cfg);
        assert!(rules
            .iter()
            .any(|r| r.scope == "bash" && r.action == PolicyAction::Prompt));
        assert!(rules
            .iter()
            .any(|r| r.pattern == "*.env" && r.action == PolicyAction::Block));
    }

    #[test]
    fn converts_permission() {
        let value = serde_json::json!({
            "bash": { "*": "ask", "git *": "allow" }
        });
        let rules = ruleset_permission(&value);
        assert!(rules
            .iter()
            .any(|r| r.scope == "bash" && r.action == PolicyAction::Prompt));
        assert!(rules
            .iter()
            .any(|r| r.pattern == "git *" && r.action == PolicyAction::Permit));
    }
}
