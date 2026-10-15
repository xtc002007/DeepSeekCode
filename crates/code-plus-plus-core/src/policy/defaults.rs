//! Default agent policy rulesets.

use code_plus_plus_schema::agent::AgentKind;
use code_plus_plus_schema::policy::{PolicyAction, PolicyRule, PolicyRuleset};

pub fn agent_default_ruleset(agent: &AgentKind) -> PolicyRuleset {
    tracing::debug!(
        target: code_plus_plus_telemetry::AUDIT,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        agent = ?agent,
        "agent_default_ruleset built"
    );
    let mut rules = base_defaults();
    match agent {
        AgentKind::Build => {
            rules.push(PolicyRule {
                scope: "question".into(),
                pattern: "*".into(),
                action: PolicyAction::Permit,
            });
        }
        AgentKind::Plan => {
            rules.extend(plan_defaults());
        }
        AgentKind::General => {
            rules.push(PolicyRule {
                scope: "task".into(),
                pattern: "*".into(),
                action: PolicyAction::Block,
            });
        }
    }
    rules
}

fn base_defaults() -> PolicyRuleset {
    vec![
        // Fail-closed default: unknown tools prompt (never silent permit).
        // Previously `*|*=>permit` was fail-open.
        PolicyRule {
            scope: "*".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        PolicyRule {
            scope: "loop_guard".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        // Sensitive paths: keep `~/` canonical form AND expanded absolute
        // form (dual evaluation in `pattern_matches` covers HOME drift and
        // startup-absolute staleness). Trailing `*` matches children.
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: "~/.ssh*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: expand_ssh_gnupg("~/.ssh*"),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: "~/.gnupg*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: expand_ssh_gnupg("~/.gnupg*"),
            action: PolicyAction::Block,
        },
        // Mutating scopes default to prompt (fail-closed).
        PolicyRule {
            scope: "bash".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        PolicyRule {
            scope: "edit".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        PolicyRule {
            scope: "write".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        PolicyRule {
            scope: "apply_patch".into(),
            pattern: "*".into(),
            action: PolicyAction::Prompt,
        },
        // Read-only whitelist: permit without prompting.
        PolicyRule {
            scope: "read".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "list".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "glob".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "grep".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "question".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "read".into(),
            pattern: "*.env".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "read".into(),
            pattern: "*.env.*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "read".into(),
            pattern: "*.env.example".into(),
            action: PolicyAction::Permit,
        },
        // Fail-closed sentinel: unparsable inputs never permit via `*`.
        PolicyRule {
            scope: "*".into(),
            pattern: "<unparsable>".into(),
            action: PolicyAction::Prompt,
        },
    ]
}

fn plan_defaults() -> PolicyRuleset {
    // Spec: block edit/write/bash/apply_patch OUTSIDE .code-plus-plus/plan/**.
    // apply_patch has an explicit scope (P0-8: previously only covered
    // indirectly via tool_scope->edit, so custom configs addressing
    // `apply_patch` directly diverged).
    vec![
        PolicyRule {
            scope: "edit".into(),
            pattern: "*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "write".into(),
            pattern: "*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "bash".into(),
            pattern: "*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "apply_patch".into(),
            pattern: "*".into(),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "edit".into(),
            pattern: ".code-plus-plus/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "write".into(),
            pattern: ".code-plus-plus/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "bash".into(),
            pattern: ".code-plus-plus/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "apply_patch".into(),
            pattern: ".code-plus-plus/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "question".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "plan".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
        },
    ]
}

fn expand_ssh_gnupg(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = code_plus_plus_types::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    path.to_string()
}
