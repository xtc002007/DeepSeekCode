//! Extract policy scopes and patterns from tool invocations.

use deepseekcode_schema::policy::PolicyRuleset;
use serde_json::Value;
use std::path::Path;

/// Map a tool name to its primary policy scope.
pub fn tool_scope(tool_name: &str) -> String {
    match tool_name {
        "write" | "edit" | "apply_patch" => "edit".into(),
        "read" => "read".into(),
        "bash" => "bash".into(),
        "powershell" => "bash".into(),
        "grep" => "grep".into(),
        "glob" => "glob".into(),
        "list" => "list".into(),
        "webfetch" => "webfetch".into(),
        "websearch" => "websearch".into(),
        "skill" => "skill".into(),
        "task" => "task".into(),
        "lsp" => "lsp".into(),
        "question" => "question".into(),
        "plan_complete" => "plan".into(),
        "todowrite" | "todo_write" | "todoread" => "edit".into(),
        _ if tool_name.starts_with("mcp_") => "mcp".into(),
        _ => tool_name.to_string(),
    }
}

/// Patterns used for ruleset evaluation for a tool call.
pub fn extract_patterns(tool_name: &str, input: &Value) -> Vec<String> {
    extract_patterns_for_workspace(tool_name, input, None)
}

/// Like [`extract_patterns`], but normalizes filesystem paths against `working_dir`.
pub fn extract_patterns_for_workspace(
    tool_name: &str,
    input: &Value,
    working_dir: Option<&str>,
) -> Vec<String> {
    let scope = tool_scope(tool_name);
    let mut patterns = match scope.as_str() {
        "bash" => bash_patterns(input),
        "read" | "edit" => path_patterns(input),
        "grep" => grep_patterns(input),
        "glob" => glob_patterns(input),
        "list" => path_patterns(input),
        "webfetch" => url_patterns(input),
        "websearch" => query_patterns(input),
        "skill" => skill_patterns(input),
        "task" => task_patterns(input),
        "lsp" => lsp_patterns(input),
        "question" => vec!["*".into()],
        "plan" => vec!["*".into()],
        _ => vec![tool_name.to_string(), "*".into()],
    };
    if let Some(dir) = working_dir {
        if matches!(scope.as_str(), "read" | "edit" | "list" | "glob" | "grep") {
            patterns = patterns
                .into_iter()
                .map(|p| {
                    if p == "*" {
                        p
                    } else {
                        crate::project_context::policy_path_pattern(dir, &p)
                    }
                })
                .collect();
        }
    }
    patterns.push("*".into());
    patterns.sort();
    patterns.dedup();
    patterns
}

/// Patterns suggested when the user chooses project-level remember.
pub fn remember_patterns(tool_name: &str, input: &Value) -> Vec<String> {
    remember_patterns_for_workspace(tool_name, input, None)
}

pub fn remember_patterns_for_workspace(
    tool_name: &str,
    input: &Value,
    working_dir: Option<&str>,
) -> Vec<String> {
    let patterns = match tool_scope(tool_name).as_str() {
        "bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            let prefix = bash_command_prefix(cmd);
            if prefix.is_empty() {
                vec!["*".into()]
            } else {
                vec![format!("{prefix} *"), prefix]
            }
        }
        "read" | "edit" | "list" | "glob" | "grep" => path_patterns(input),
        _ => vec!["*".into()],
    };
    if let Some(dir) = working_dir {
        return patterns
            .into_iter()
            .map(|p| {
                if p == "*" {
                    p
                } else {
                    crate::project_context::policy_path_pattern(dir, &p)
                }
            })
            .collect();
    }
    patterns
}

/// Paths in tool input that fall outside the workspace.
pub fn outside_workspace_paths(working_dir: &str, input: &Value, tool_name: &str) -> Vec<String> {
    if tool_scope(tool_name) == "bash" {
        if let Some(cmd) = input["command"].as_str() {
            if crate::project_context::command_escapes_project_directory(cmd, working_dir).is_some()
            {
                return vec!["*".into()];
            }
        }
        return vec![];
    }

    let mut paths = Vec::new();
    collect_paths_from_input(input, tool_name, &mut paths);
    let root = Path::new(working_dir);
    paths
        .into_iter()
        .filter(|p| is_outside_workspace(root, p))
        .collect()
}

fn bash_patterns(input: &Value) -> Vec<String> {
    let cmd = input["command"].as_str().unwrap_or("");
    let mut patterns = vec![cmd.to_string()];
    let prefix = bash_command_prefix(cmd);
    if !prefix.is_empty() {
        patterns.push(format!("{prefix} *"));
        patterns.push(prefix);
    }
    patterns
}

fn bash_command_prefix(cmd: &str) -> String {
    cmd.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

fn path_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["path", "file", "file_path", "target"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            out.push(p.to_string());
        }
    }
    if out.is_empty() {
        out.push("*".into());
    }
    out
}

fn grep_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = input["pattern"].as_str() {
        out.push(p.to_string());
    }
    out.extend(path_patterns(input));
    out
}

fn glob_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = input
        .get("pattern")
        .or_else(|| input.get("glob_pattern"))
        .and_then(|v| v.as_str())
    {
        out.push(p.to_string());
    }
    out
}

fn url_patterns(input: &Value) -> Vec<String> {
    if let Some(url) = input["url"].as_str() {
        vec![url.to_string()]
    } else {
        vec!["*".into()]
    }
}

fn query_patterns(input: &Value) -> Vec<String> {
    if let Some(q) = input["query"].as_str().or_else(|| input["search"].as_str()) {
        vec![q.to_string()]
    } else {
        vec!["*".into()]
    }
}

fn skill_patterns(input: &Value) -> Vec<String> {
    if let Some(name) = input["name"].as_str().or_else(|| input["skill"].as_str()) {
        vec![name.to_string()]
    } else {
        vec!["*".into()]
    }
}

fn task_patterns(input: &Value) -> Vec<String> {
    if let Some(agent) = input["agent"]
        .as_str()
        .or_else(|| input["subagent"].as_str())
    {
        vec![agent.to_string()]
    } else {
        vec!["*".into()]
    }
}

fn lsp_patterns(input: &Value) -> Vec<String> {
    if let Some(op) = input["operation"].as_str() {
        vec![op.to_string()]
    } else {
        vec!["*".into()]
    }
}

fn collect_paths_from_input(input: &Value, tool_name: &str, out: &mut Vec<String>) {
    match tool_scope(tool_name).as_str() {
        "bash" => {
            if let Some(cmd) = input["command"].as_str() {
                for token in cmd.split_whitespace() {
                    if looks_like_path(token) {
                        out.push(token.to_string());
                    }
                }
            }
        }
        _ => {
            out.extend(path_patterns(input));
        }
    }
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('\\')
}

fn is_outside_workspace(root: &Path, path: &str) -> bool {
    let project_dir = root.to_string_lossy();
    let normalized = crate::project_context::policy_path_pattern(&project_dir, path);
    if normalized == "*" || normalized == "." {
        return false;
    }
    !crate::project_context::path_within_project(&project_dir, Path::new(&normalized))
}

/// Fingerprint for loop-guard detection.
pub fn tool_fingerprint(tool_name: &str, input: &Value) -> String {
    format!("{tool_name}:{}", input)
}

#[allow(dead_code)]
pub fn grants_to_ruleset(grants: &[(String, String)]) -> PolicyRuleset {
    grants
        .iter()
        .map(|(scope, pattern)| deepseekcode_schema::policy::PolicyRule {
            scope: scope.clone(),
            pattern: pattern.clone(),
            action: deepseekcode_schema::policy::PolicyAction::Permit,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_patterns_include_command_prefix() {
        let patterns = extract_patterns("bash", &json!({ "command": "git status" }));
        assert!(patterns.iter().any(|p| p.contains("git")));
    }

    #[test]
    fn absolute_path_normalizes_for_plan_policy() {
        let dir = std::env::temp_dir().join("deepseekcode-policy-path-test");
        let _ = std::fs::create_dir_all(&dir);
        let abs = dir
            .join(".deepseekcode")
            .join("plan")
            .join("foo")
            .join("PLAN.md");
        let pattern = crate::project_context::policy_path_pattern(
            &dir.to_string_lossy(),
            &abs.to_string_lossy(),
        );
        assert_eq!(pattern, ".deepseekcode/plan/foo/PLAN.md");
    }

    #[test]
    fn read_patterns_use_path_field() {
        let patterns = extract_patterns("read", &json!({ "path": "src/main.rs" }));
        assert!(patterns.iter().any(|p| p == "src/main.rs"));
    }

    #[test]
    fn bash_cd_to_project_root_is_not_outside_workspace() {
        let dir = std::env::temp_dir().join("deepseekcode-bash-policy-test");
        let _ = std::fs::create_dir_all(&dir);
        let cmd = format!("cd /d \"{}\" && npx next build 2>&1", dir.display());
        let outside =
            outside_workspace_paths(&dir.to_string_lossy(), &json!({ "command": cmd }), "bash");
        assert!(outside.is_empty(), "unexpected outside paths: {outside:?}");
    }

    #[test]
    fn bash_cd_outside_project_is_flagged() {
        let dir = std::env::temp_dir().join("deepseekcode-bash-policy-test-2");
        let _ = std::fs::create_dir_all(&dir);
        let outside = outside_workspace_paths(
            &dir.to_string_lossy(),
            &json!({ "command": "cd /d C:\\\\Windows && dir" }),
            "bash",
        );
        assert!(!outside.is_empty());
    }

    #[test]
    fn tool_scope_maps_write_to_edit() {
        assert_eq!(tool_scope("write"), "edit");
        assert_eq!(tool_scope("bash"), "bash");
    }
}
