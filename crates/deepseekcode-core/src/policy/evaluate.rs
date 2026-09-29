//! Pattern matching and ruleset evaluation for the policy engine.

use deepseekcode_schema::policy::{PolicyAction, PolicyDecision, PolicyRule, PolicyRuleset};

/// Merge rulesets; later entries win during evaluation (last-match).
pub fn merge_rulesets(sets: impl IntoIterator<Item = PolicyRuleset>) -> PolicyRuleset {
    sets.into_iter().flatten().collect()
}

/// Evaluate a single scope/pattern against merged rulesets (last matching rule wins).
pub fn evaluate_rule(scope: &str, pattern: &str, ruleset: &PolicyRuleset) -> PolicyRule {
    ruleset
        .iter()
        .rev()
        .find(|rule| scope_matches(&rule.scope, scope) && pattern_matches(&rule.pattern, pattern))
        .cloned()
        .unwrap_or(PolicyRule {
            scope: scope.to_string(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        })
}

/// Evaluate all patterns; any Block wins, else any Prompt, else Permit.
/// When concrete patterns exist (non-`*`), the catch-all `*` is ignored so
/// scoped permits like `.deepseekcode/plan/**` are not overridden.
pub fn evaluate_patterns(
    scope: &str,
    patterns: &[String],
    ruleset: &PolicyRuleset,
) -> PolicyDecision {
    tracing::debug!(
        target: deepseekcode_telemetry::AUDIT,
        run_id = %deepseekcode_telemetry::current_run_id(),
        scope = %scope,
        patterns = ?patterns,
        rules = ruleset.len(),
        "policy evaluate started"
    );
    let concrete: Vec<&String> = patterns.iter().filter(|p| *p != "*").collect();
    let eval_patterns: Vec<&String> = if concrete.is_empty() {
        patterns.iter().collect()
    } else {
        concrete
    };
    let mut saw_prompt = false;
    for pattern in eval_patterns {
        let rule = evaluate_rule(scope, pattern, ruleset);
        match rule.action {
            PolicyAction::Block => {
                tracing::info!(
                    target: deepseekcode_telemetry::AUDIT,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    scope = %scope,
                    decision = "block",
                    "policy evaluate decided"
                );
                return PolicyDecision::Block;
            }
            PolicyAction::Prompt => saw_prompt = true,
            PolicyAction::Permit => {}
        }
    }
    let decision = if saw_prompt {
        PolicyDecision::Prompt
    } else {
        PolicyDecision::Permit
    };
    tracing::debug!(
        target: deepseekcode_telemetry::AUDIT,
        run_id = %deepseekcode_telemetry::current_run_id(),
        scope = %scope,
        decision = ?decision,
        "policy evaluate decided"
    );
    decision
}

pub fn scope_matches(rule_scope: &str, scope: &str) -> bool {
    if rule_scope == "*" || rule_scope == scope {
        return true;
    }
    if rule_scope.starts_with("mcp:") {
        return mcp_scope_matches(rule_scope, scope);
    }
    wildcard_match(rule_scope, scope)
}

pub fn pattern_matches(rule_pattern: &str, pattern: &str) -> bool {
    if rule_pattern == "*" || rule_pattern == pattern {
        return true;
    }
    wildcard_match(rule_pattern, pattern)
}

fn wildcard_match(rule: &str, value: &str) -> bool {
    if rule.contains('*') || rule.contains('?') || rule.contains('[') {
        return glob::Pattern::new(rule)
            .map(|p| p.matches(value))
            .unwrap_or(false);
    }
    if let Some(prefix) = rule.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    false
}

fn mcp_scope_matches(rule_scope: &str, scope: &str) -> bool {
    let Some(rest) = rule_scope.strip_prefix("mcp:") else {
        return false;
    };
    let mut parts = rest.splitn(2, ':');
    let server = parts.next().unwrap_or("");
    let tool = parts.next().unwrap_or("");
    if server.is_empty() {
        return false;
    }
    if tool.is_empty() || tool == "*" {
        return scope.starts_with(&format!("mcp_{server}_"))
            || scope.starts_with(&format!("{server}_"));
    }
    scope == format!("mcp_{server}_{tool}") || scope == format!("{server}_{tool}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_rule_wins() {
        let ruleset = vec![
            PolicyRule {
                scope: "bash".into(),
                pattern: "*".into(),
                action: PolicyAction::Prompt,
            },
            PolicyRule {
                scope: "bash".into(),
                pattern: "git *".into(),
                action: PolicyAction::Permit,
            },
        ];
        assert_eq!(
            evaluate_rule("bash", "git status", &ruleset).action,
            PolicyAction::Permit
        );
        assert_eq!(
            evaluate_rule("bash", "rm -rf /", &ruleset).action,
            PolicyAction::Prompt
        );
    }

    #[test]
    fn block_overrides_prompt() {
        let ruleset = vec![PolicyRule {
            scope: "read".into(),
            pattern: "*.env".into(),
            action: PolicyAction::Block,
        }];
        assert_eq!(
            evaluate_patterns("read", &["src/main.rs".into(), ".env".into()], &ruleset),
            PolicyDecision::Block
        );
    }

    #[test]
    fn concrete_path_skips_catchall_star() {
        let ruleset = vec![
            PolicyRule {
                scope: "edit".into(),
                pattern: "*".into(),
                action: PolicyAction::Block,
            },
            PolicyRule {
                scope: "edit".into(),
                pattern: ".deepseekcode/plan/**".into(),
                action: PolicyAction::Permit,
            },
        ];
        assert_eq!(
            evaluate_patterns(
                "edit",
                &[".deepseekcode/plan/foo/PLAN.md".into(), "*".into()],
                &ruleset
            ),
            PolicyDecision::Permit
        );
        assert_eq!(
            evaluate_patterns("edit", &["src/main.rs".into(), "*".into()], &ruleset),
            PolicyDecision::Block
        );
    }
}
