//! Default agent policy rulesets.

use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::policy::{PolicyAction, PolicyRule, PolicyRuleset};

pub fn agent_default_ruleset(agent: &AgentKind) -> PolicyRuleset {
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
        PolicyRule {
            scope: "*".into(),
            pattern: "*".into(),
            action: PolicyAction::Permit,
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
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: expand_ssh_gnupg("~/.ssh"),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "outside_workspace".into(),
            pattern: expand_ssh_gnupg("~/.gnupg"),
            action: PolicyAction::Block,
        },
        PolicyRule {
            scope: "read".into(),
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
    ]
}

fn plan_defaults() -> PolicyRuleset {
    // Spec: block edit/write/bash/apply_patch OUTSIDE .deepseekcode/plan/**.
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
            pattern: ".deepseekcode/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "write".into(),
            pattern: ".deepseekcode/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "bash".into(),
            pattern: ".deepseekcode/plan/**".into(),
            action: PolicyAction::Permit,
        },
        PolicyRule {
            scope: "apply_patch".into(),
            pattern: ".deepseekcode/plan/**".into(),
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
        if let Some(home) = deepseekcode_types::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    path.to_string()
}
