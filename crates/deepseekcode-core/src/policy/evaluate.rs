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
    let mut saw_exact_permit = false;
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
            PolicyAction::Permit => {
                // Exact grant (e.g. Once `echo once`) covers the turn even when
                // derived prefix views (`echo *`, `echo`) still map to Prompt.
                // Without this, fail-closed defaults would require the grant to
                // enumerate every derived view, forcing over-broad `prefix *`
                // grants (e.g. `git *` covering `git rm`) to make same-command
                // retries pass. Exact-permit wins over derived-prompt, but never
                // over Block (handled above).
                if rule.pattern == **pattern {
                    saw_exact_permit = true;
                }
            }
        }
    }
    let decision = if saw_exact_permit {
        PolicyDecision::Permit
    } else if saw_prompt {
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
    // Dual evaluation for `~/` canonical form vs expanded absolute form:
    // a rule written as `~/.ssh*` must match an extracted absolute
    // `/home/u/.ssh/id_rsa` and vice versa (HOME drift / startup staleness).
    let expanded_rule = expand_tilde_for_match(rule_pattern);
    let expanded_value = expand_tilde_for_match(pattern);
    if expanded_rule != rule_pattern || expanded_value != pattern {
        if expanded_rule == expanded_value {
            return true;
        }
        if wildcard_match(&expanded_rule, &expanded_value) {
            return true;
        }
    }
    wildcard_match(rule_pattern, pattern)
}

/// Expand `~/`, `$HOME/` for matching only (no FS access). Used so
/// `~/`-canonical rules match absolute values and vice versa.
fn expand_tilde_for_match(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = deepseekcode_types::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    if let Some(rest) = s.strip_prefix("$HOME/") {
        if let Some(home) = deepseekcode_types::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    s.to_string()
}

/// Validate a glob pattern at load/write time (fail-closed). Returns an
/// error for syntactically invalid globs instead of silently treating them
/// as non-matches (which previously fell through to a fail-open permit).
pub fn validate_glob_pattern(pattern: &str) -> Result<(), String> {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        glob::Pattern::new(pattern).map(|_| ()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn wildcard_match(rule: &str, value: &str) -> bool {
    if rule.contains('*') || rule.contains('?') || rule.contains('[') {
        // Invalid globs fail closed (no match); callers must validate at
        // load time via `validate_glob_pattern` to reject them loudly.
        return glob::Pattern::new(rule)
            .map(|p| p.matches(value))
            .unwrap_or(false);
    }
    false
}

fn mcp_scope_matches(rule_scope: &str, scope: &str) -> bool {
    // Canonical form is `mcp:<server>:<tool>` (see `tool_scope`); legacy
    // `mcp_<server>_<tool>` / `<server>_<tool>` forms are also accepted for
    // DB rows written before the fix.
    let Some(rest) = rule_scope.strip_prefix("mcp:") else {
        return false;
    };
    let mut parts = rest.splitn(2, ':');
    let server = parts.next().unwrap_or("");
    let tool = parts.next().unwrap_or("");
    if server.is_empty() {
        return false;
    }
    // Normalize the concrete scope to (server, tool) regardless of form.
    let (scope_server, scope_tool): (String, String) =
        if let Some(s) = scope.strip_prefix("mcp:") {
            let mut sp = s.splitn(2, ':');
            (
                sp.next().unwrap_or("").to_string(),
                sp.next().unwrap_or("").to_string(),
            )
        } else if let Some(s) = scope.strip_prefix("mcp_") {
            match s.split_once('_') {
                Some((srv, tl)) => (srv.to_string(), tl.to_string()),
                None => (s.to_string(), String::new()),
            }
        } else {
            match scope.split_once('_') {
                Some((srv, tl)) if srv == server => (srv.to_string(), tl.to_string()),
                _ => (String::new(), String::new()),
            }
        };
    if scope_server != server {
        // Fallback to legacy prefix matching for mixed-form rules.
        if tool.is_empty() || tool == "*" {
            return scope.starts_with(&format!("mcp_{server}_"))
                || scope.starts_with(&format!("{server}_"))
                || scope.starts_with(&format!("mcp:{server}:"))
                || scope == format!("mcp:{server}")
                || scope == format!("mcp:{server}:*");
        }
        return false;
    }
    if tool.is_empty() || tool == "*" {
        return true;
    }
    // Support trailing wildcards in tool part (e.g. `mcp:gh:search*`).
    if tool.contains('*') || tool.contains('?') || tool.contains('[') {
        return glob::Pattern::new(tool)
            .map(|p| p.matches(&scope_tool))
            .unwrap_or(false);
    }
    scope_tool == tool
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
