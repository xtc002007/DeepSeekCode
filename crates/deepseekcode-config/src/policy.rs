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

/// Legacy `permissions.allow/deny` → policy rules.
pub fn ruleset_from_permissions(permissions: &PermissionsConfig) -> PolicyRuleset {
    let mut rules = Vec::new();
    for pattern in &permissions.deny {
        let scope = legacy_pattern_to_scope(pattern);
        rules.push(PolicyRule {
            scope,
            pattern: "*".into(),
            action: PolicyAction::Block,
        });
    }
    for pattern in &permissions.allow {
        let scope = legacy_pattern_to_scope(pattern);
        rules.push(PolicyRule {
            scope,
            pattern: "*".into(),
            action: PolicyAction::Permit,
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

fn legacy_pattern_to_scope(pattern: &str) -> String {
    if pattern == "*" {
        return "*".into();
    }
    if pattern.starts_with("mcp:") {
        return pattern.to_string();
    }
    pattern.to_string()
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
