//! Extract policy scopes and patterns from tool invocations.

use code_plus_plus_schema::policy::PolicyRuleset;
use serde_json::Value;
use std::path::Path;

/// Map a tool name to its primary policy scope.
/// P0-8: `apply_patch` keeps its own scope (plan defaults now block it
/// explicitly); `tool_result_page` is `read` (paged recovery for large
/// outputs must work in plan mode, which has no `task` tool).
/// MCP bridged tools (`mcp_<server>_<tool>`) keep full granularity as
/// `mcp:<server>:<tool>` so `mcp:server:*` rules can hit (previously
/// collapsed to `mcp` and never matched `mcp_scope_matches`).
pub fn tool_scope(tool_name: &str) -> String {
    match tool_name {
        "write" | "edit" => "edit".into(),
        "apply_patch" => "apply_patch".into(),
        "read" | "tool_result_page" => "read".into(),
        "bash" | "powershell" | "shell" => "bash".into(),
        "grep" => "grep".into(),
        "glob" => "glob".into(),
        "list" => "list".into(),
        "webfetch" => "webfetch".into(),
        "websearch" => "websearch".into(),
        "skill" => "skill".into(),
        "task" => "task".into(),
        "lsp" => "lsp".into(),
        "question" => "question".into(),
        "plan_complete" | "plan.complete" | "plan" => "plan".into(),
        "todowrite" | "todo_write" => "edit".into(),
        // todoread is read-only: must not inherit edit blocks.
        "todoread" | "todo_read" => "read".into(),
        "todo" => "edit".into(),
        "use_capability" => "task".into(),
        _ if tool_name.starts_with("mcp_") => mcp_tool_scope(tool_name),
        _ if tool_name.starts_with("mcp:") => tool_name.to_string(),
        _ => tool_name.to_string(),
    }
}

/// `mcp_github_search_issues` -> `mcp:github:search_issues`.
/// Server is the first segment after `mcp_`; remainder is the tool name
/// (may contain underscores). Bare `mcp_<server>` maps to `mcp:<server>:*`.
fn mcp_tool_scope(tool_name: &str) -> String {
    let rest = tool_name.strip_prefix("mcp_").unwrap_or(tool_name);
    match rest.split_once('_') {
        Some((server, tool)) if !server.is_empty() && !tool.is_empty() => {
            format!("mcp:{server}:{tool}")
        }
        _ if !rest.is_empty() => format!("mcp:{rest}:*"),
        _ => "mcp".into(),
    }
}

/// Sentinel for unparsable/missing inputs: must never be permitted via a
/// `*` whitelist. Base defaults map `*|<unparsable>=>prompt` last so this
/// evaluates to Prompt (fail-closed) instead of Permit.
pub const UNPARSABLE_PATTERN: &str = "<unparsable>";

/// Patterns used for ruleset evaluation for a tool call.
pub fn extract_patterns(tool_name: &str, input: &Value) -> Vec<String> {
    tracing::debug!(
        target: code_plus_plus_telemetry::AUDIT,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        tool = %tool_name,
        "extract_patterns requested"
    );
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
        s if s.starts_with("mcp:") || s == "mcp" => vec![tool_name.to_string(), scope.clone()],
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
        "question" => question_patterns(input),
        "plan" => plan_patterns(input),
        _ => vec![tool_name.to_string(), "*".into()],
    };
    if let Some(dir) = working_dir {
        if matches!(scope.as_str(), "read" | "edit" | "list" | "glob" | "grep") {
            patterns = patterns
                .into_iter()
                .map(|p| {
                    if p == "*" || p == UNPARSABLE_PATTERN {
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
    let scope = tool_scope(tool_name);
    let patterns = match scope.as_str() {
        "bash" => {
            // Narrowed to the exact command (truncated): a `once` grant only
            // re-permits the identical command, never `<2-token prefix> *`.
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .or_else(|| input.get("cmd").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim();
            if cmd.is_empty() {
                vec![UNPARSABLE_PATTERN.into()]
            } else {
                let mut out = vec![truncate_exact(cmd, 500)];
                // Include decoded payloads so grants are bound to the real
                // payload, not just the wrapper (`-c`, `-enc`).
                for extra in bash_extra_payloads(cmd) {
                    out.push(truncate_exact(&extra, 500));
                }
                out
            }
        }
        "read" | "edit" | "list" | "glob" | "grep" => {
            // Exact relative paths only (already normalized by caller when
            // `working_dir` is present); never wildcard prefixes.
            let mut out = path_patterns(input);
            out.retain(|p| p != "*");
            if out.is_empty() {
                vec![UNPARSABLE_PATTERN.into()]
            } else {
                out
            }
        }
        // Scoped remembers: never fall back to global `*` permit.
        "question" => vec!["question".into()],
        "plan" => vec!["plan".into()],
        "skill" => skill_patterns(input),
        "task" => task_patterns(input),
        "lsp" => lsp_patterns(input),
        "webfetch" => url_patterns(input),
        "websearch" => query_patterns(input),
        s if s.starts_with("mcp:") || s == "mcp" => vec![tool_name.to_string()],
        _ => vec![tool_name.to_string()],
    };
    if let Some(dir) = working_dir {
        // Only path scopes are normalized; bash exact commands / MCP names /
        // question markers must not pass through path normalization.
        let is_path_scope = matches!(scope.as_str(), "read" | "edit" | "list" | "glob" | "grep");
        return patterns
            .into_iter()
            .map(|p| {
                if p == "*" || p == UNPARSABLE_PATTERN || !is_path_scope {
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
    tracing::debug!(
        target: code_plus_plus_telemetry::AUDIT,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        tool = %tool_name,
        working_dir = %working_dir,
        "outside_workspace_paths checked"
    );
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
    let cmd = input
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("cmd").and_then(|v| v.as_str()))
        .unwrap_or("");
    if cmd.trim().is_empty() {
        return vec![UNPARSABLE_PATTERN.into()];
    }
    let mut patterns = vec![truncate_exact(cmd.trim(), 500)];
    let prefix = bash_command_prefix(cmd);
    if !prefix.is_empty() {
        patterns.push(format!("{prefix} *"));
        patterns.push(prefix);
    }
    // Decode `-c` payloads and Base64/`-EncodedCommand` wrappers so policy
    // sees the real payload, not just the wrapper.
    for extra in bash_extra_payloads(cmd) {
        patterns.push(truncate_exact(&extra, 500));
    }
    patterns
}

/// Extract decoded payloads from `bash -c "..."`, `sh -c '...'`,
/// `powershell -EncodedCommand <b64>` / `-enc <b64>`, and long Base64 tokens.
fn bash_extra_payloads(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let tokens = shell_tokens(cmd);
    let mut i = 0;
    while i < tokens.len() {
        let lower = tokens[i].to_ascii_lowercase();
        if (lower == "-c" || lower == "/c") && i + 1 < tokens.len() {
            let payload = tokens[i + 1].trim_matches(|c| c == '"' || c == '\'');
            if !payload.is_empty() {
                out.push(payload.to_string());
                // Recurse: payload may itself contain an encoded wrapper.
                for inner in bash_extra_payloads(payload) {
                    out.push(inner);
                }
            }
            i += 2;
            continue;
        }
        if (lower == "-encodedcommand"
            || lower == "-enc"
            || lower == "-e"
            || lower == "--encodedcommand")
            && i + 1 < tokens.len()
        {
            let b64 = tokens[i + 1].trim_matches(|c| c == '"' || c == '\'');
            if let Some(decoded) = try_base64_decode(b64) {
                out.push(decoded);
            } else {
                out.push(b64.to_string());
            }
            i += 2;
            continue;
        }
        // Bare long Base64 blob (e.g. `echo <b64> | base64 -d | sh`): decode
        // opportunistically when it looks like Base64 and decodes to text.
        let t = tokens[i].trim_matches(|c| c == '"' || c == '\'');
        if t.len() >= 32 && is_plausible_base64(t) {
            if let Some(decoded) = try_base64_decode(t) {
                out.push(decoded);
            }
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out
}

#[allow(clippy::manual_is_multiple_of)]
// MSRV 1.85: `usize::is_multiple_of` stabilized in 1.87, keep `%` form.
fn is_plausible_base64(s: &str) -> bool {
    !s.is_empty()
        && s.len() % 4 == 0
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

fn try_base64_decode(s: &str) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    // PowerShell uses UTF-16LE; try UTF-8 first, then UTF-16LE.
    let bytes = STANDARD.decode(s).ok()?;
    if bytes.is_empty() || bytes.len() > 8192 {
        return None;
    }
    if let Ok(text) = String::from_utf8(bytes.clone()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() && trimmed.chars().all(|c| !c.is_control() || c.is_whitespace()) {
            return Some(trimmed.to_string());
        }
    }
    // UTF-16LE fallback (PowerShell `-EncodedCommand`).
    if bytes.len() % 2 == 0 {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(text) = String::from_utf16(&u16s) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn truncate_exact(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

fn bash_command_prefix(cmd: &str) -> String {
    // Quote-aware first-two-tokens: `git commit -m "a b"` -> `git commit`,
    // `"C:\Program Files\git.exe" status` keeps quoted binary intact.
    let tokens = shell_tokens(cmd);
    tokens.into_iter().take(2).collect::<Vec<_>>().join(" ")
}

fn shell_tokens(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in cmd.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(cur.clone());
                    cur.clear();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn path_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "path",
        "file",
        "file_path",
        "target",
        "dir",
        "directory",
        "cwd",
        "working_directory",
    ] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            if !p.trim().is_empty() {
                out.push(p.to_string());
            }
        }
    }
    // ApplyPatch carries paths inside the unified diff, not top-level keys.
    if let Some(patch) = input.get("patch").and_then(|v| v.as_str()) {
        out.extend(crate::tools::parse_patch_paths(patch));
    }
    if out.is_empty() {
        out.push(UNPARSABLE_PATTERN.into());
    }
    out
}

fn grep_patterns(input: &Value) -> Vec<String> {
    // Policy globs match FILES, never the search string itself.
    // Including `pattern` (the query) caused query text to be evaluated
    // against path rules (false blocks/permits).
    path_patterns(input)
}

fn glob_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = input
        .get("pattern")
        .or_else(|| input.get("glob_pattern"))
        .and_then(|v| v.as_str())
    {
        if !p.trim().is_empty() {
            out.push(p.to_string());
        }
    }
    // Glob also honors explicit path/cwd scoping (previously ignored, so
    // `glob` with `path: /etc` bypassed path rules).
    for key in ["path", "cwd", "directory", "dir", "working_directory"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            if !p.trim().is_empty() {
                out.push(p.to_string());
            }
        }
    }
    if out.is_empty() {
        out.push(UNPARSABLE_PATTERN.into());
    }
    out
}

fn url_patterns(input: &Value) -> Vec<String> {
    if let Some(url) = input["url"].as_str() {
        if !url.trim().is_empty() {
            return vec![url.to_string()];
        }
        vec![UNPARSABLE_PATTERN.into()]
    } else {
        vec![UNPARSABLE_PATTERN.into()]
    }
}

fn query_patterns(input: &Value) -> Vec<String> {
    if let Some(q) = input["query"].as_str().or_else(|| input["search"].as_str()) {
        if !q.trim().is_empty() {
            return vec![q.to_string()];
        }
        vec![UNPARSABLE_PATTERN.into()]
    } else {
        vec![UNPARSABLE_PATTERN.into()]
    }
}

fn skill_patterns(input: &Value) -> Vec<String> {
    if let Some(name) = input["name"].as_str().or_else(|| input["skill"].as_str()) {
        if !name.trim().is_empty() {
            return vec![name.to_string()];
        }
        vec![UNPARSABLE_PATTERN.into()]
    } else {
        vec![UNPARSABLE_PATTERN.into()]
    }
}

fn task_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(agent) = input["agent"]
        .as_str()
        .or_else(|| input["subagent"].as_str())
    {
        if !agent.trim().is_empty() {
            out.push(agent.to_string());
        }
    }
    // Include the prompt hash so distinct prompts are distinct patterns
    // (previously only the agent name was evaluated, so any prompt for the
    // same agent shared one grant).
    for key in ["prompt", "task", "description", "instructions"] {
        if let Some(text) = input.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                out.push(format!("prompt-hash:{:08x}:len={}", simple_prompt_hash(text), text.chars().count()));
                // Also include a truncated prefix for human-written rules.
                let prefix: String = text.chars().take(64).collect();
                out.push(format!("prompt:{prefix}"));
                break;
            }
        }
    }
    if out.is_empty() {
        out.push(UNPARSABLE_PATTERN.into());
    }
    out
}

fn simple_prompt_hash(s: &str) -> u32 {
    s.bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
}

fn question_patterns(input: &Value) -> Vec<String> {
    // Content-derived patterns so `question|spam*=>block` can target text
    // (previously constant `*`). The `question|*=>permit` whitelist still
    // matches these via `*`, preserving the default permit.
    let mut out = Vec::new();
    for key in ["question", "text", "prompt", "content"] {
        if let Some(text) = input.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                out.push(truncate_exact(text.trim(), 200));
                break;
            }
        }
    }
    if out.is_empty() {
        // No content field (e.g. unit tests with `{}`): keep wildcard so the
        // read-only whitelist still permits without prompting.
        out.push("*".into());
    } else {
        out.push("*".into());
    }
    out
}

fn plan_patterns(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["plan", "content", "text", "prompt"] {
        if let Some(text) = input.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                out.push(truncate_exact(text.trim(), 200));
                break;
            }
        }
    }
    // Catch-all always present so unscoped callers fail to Prompt, not panic.
    out.push("*".into());
    out
}

fn lsp_patterns(input: &Value) -> Vec<String> {
    // LspTool schema uses `command`+`method`+`params`; legacy `operation`
    // kept as fallback so method-granular rules actually hit.
    if let Some(m) = input
        .get("method")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("operation").and_then(|v| v.as_str()))
    {
        if !m.trim().is_empty() {
            return vec![m.to_string()];
        }
    }
    vec![UNPARSABLE_PATTERN.into()]
}

fn collect_paths_from_input(input: &Value, tool_name: &str, out: &mut Vec<String>) {
    match tool_scope(tool_name).as_str() {
        "bash" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .or_else(|| input.get("cmd").and_then(|v| v.as_str()))
                .unwrap_or("");
            // Quote-aware tokenization + redirection targets (`> ../out`,
            // `2>file`) + `~`/`$HOME`/`..` so escapes can't hide.
            for token in shell_tokens(cmd) {
                let t = token.trim_matches(|c| c == '"' || c == '\'').trim();
                let t = t.trim_start_matches(['>', '<']);
                if t.is_empty() {
                    continue;
                }
                if looks_like_path(t) {
                    out.push(t.to_string());
                }
            }
        }
        _ => {
            out.extend(path_patterns(input));
        }
    }
}

fn looks_like_path(token: &str) -> bool {
    let t = token.trim_matches(|c| c == '"' || c == '\'');
    t.starts_with('/')
        || t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with('~')
        || t.starts_with("$HOME")
        || t.starts_with("%")
        || t.contains('\\')
        || t.contains('/')
        || t.contains("..")
}

fn is_outside_workspace(root: &Path, path: &str) -> bool {
    let project_dir = root.to_string_lossy();
    let normalized = crate::project_context::policy_path_pattern(&project_dir, path);
    if normalized == "*" || normalized == "." {
        return false;
    }
    !crate::project_context::path_within_project(&project_dir, Path::new(&normalized))
}

/// Fingerprint for loop-guard detection (canonical: sorted keys, trimmed).
pub fn tool_fingerprint(tool_name: &str, input: &Value) -> String {
    code_plus_plus_types::tool_fingerprint_canonical(tool_name, input)
}

#[allow(dead_code)]
pub fn grants_to_ruleset(grants: &[(String, String)]) -> PolicyRuleset {
    grants
        .iter()
        .map(|(scope, pattern)| code_plus_plus_schema::policy::PolicyRule {
            scope: scope.clone(),
            pattern: pattern.clone(),
            action: code_plus_plus_schema::policy::PolicyAction::Permit,
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
        let dir = std::env::temp_dir().join("code-plus-plus-policy-path-test");
        let _ = std::fs::create_dir_all(&dir);
        let abs = dir
            .join(".code-plus-plus")
            .join("plan")
            .join("foo")
            .join("PLAN.md");
        let pattern = crate::project_context::policy_path_pattern(
            &dir.to_string_lossy(),
            &abs.to_string_lossy(),
        );
        assert_eq!(pattern, ".code-plus-plus/plan/foo/PLAN.md");
    }

    #[test]
    fn read_patterns_use_path_field() {
        let patterns = extract_patterns("read", &json!({ "path": "src/main.rs" }));
        assert!(patterns.iter().any(|p| p == "src/main.rs"));
    }

    #[test]
    fn bash_cd_to_project_root_is_not_outside_workspace() {
        let dir = std::env::temp_dir().join("code-plus-plus-bash-policy-test");
        let _ = std::fs::create_dir_all(&dir);
        let cmd = format!("cd /d \"{}\" && npx next build 2>&1", dir.display());
        let outside =
            outside_workspace_paths(&dir.to_string_lossy(), &json!({ "command": cmd }), "bash");
        assert!(outside.is_empty(), "unexpected outside paths: {outside:?}");
    }

    #[test]
    fn bash_cd_outside_project_is_flagged() {
        let dir = std::env::temp_dir().join("code-plus-plus-bash-policy-test-2");
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
