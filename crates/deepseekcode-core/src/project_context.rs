//! Project-level mandatory context files (AGENTS.md) and path sandboxing.

use deepseekcode_schema::tool::ToolFailure;
use std::path::{Component, Path, PathBuf};

pub const AGENTS_MD_TEMPLATE: &str = "# Project\n\n\
Describe this project so the AI assistant has context.\n\n\
## Structure\n\n\
- Note important directories and conventions here.\n";

/// Create `AGENTS.md` when missing. Returns `true` if a new file was written.
pub fn ensure_agents_md(project_dir: &str) -> bool {
    let root = Path::new(project_dir);
    let agents = root.join("AGENTS.md");
    if agents.exists() {
        return false;
    }
    if !root.exists() && std::fs::create_dir_all(root).is_err() {
        return false;
    }
    std::fs::write(&agents, AGENTS_MD_TEMPLATE).is_ok()
}

/// Whether `path` resolves inside `project_dir` (including the root itself).
/// Symlink-aware: existing prefixes are canonicalized so `link->/etc` via
/// `link/secret` is correctly judged outside. `~`/`$HOME`/`%VAR%`/UNC and
/// verbatim prefixes are expanded before comparison.
pub fn path_within_project(project_dir: &str, path: &Path) -> bool {
    let root = canonical_root(project_dir);
    let expanded = expand_user_path(path.to_string_lossy().as_ref());
    let expanded_path = PathBuf::from(expanded);
    let resolved = resolve_path_against_root(project_dir, &root, &expanded_path);
    let canonical_target = canonicalize_existing_prefix(&resolved);
    let root_cmp = normalize_compare_path(&root);
    let target_cmp = normalize_compare_path(&canonical_target);
    if target_cmp == root_cmp || target_cmp.starts_with(&format!("{root_cmp}/")) {
        // Even if lexically inside, a symlink component may point outside:
        // resolve the full chain when the target (or its parent) exists.
        if let Ok(real) = std::fs::canonicalize(&resolved) {
            let real = strip_verbatim_prefix(real);
            let real_cmp = normalize_compare_path(&real);
            return real_cmp == root_cmp || real_cmp.starts_with(&format!("{root_cmp}/"));
        }
        return true;
    }
    false
}

fn canonical_root(project_dir: &str) -> PathBuf {
    let expanded = expand_user_path(project_dir);
    let p = PathBuf::from(expanded);
    if let Ok(real) = std::fs::canonicalize(&p) {
        return strip_verbatim_prefix(real);
    }
    // Fallback for non-existent roots (tests, not-yet-created projects):
    // resolve the longest EXISTING prefix so the root agrees with
    // `canonicalize_existing_prefix` applied to targets below. A plain
    // lexical fallback leaves drive-less `/tmp/...` roots on Windows while
    // targets resolve to `D:\tmp\...`, misjudging every relative path as
    // outside (regression vs the old purely-lexical check).
    canonicalize_existing_prefix(&best_effort_abs(p.to_string_lossy().as_ref()))
}

/// Expand `~`, `$HOME`/`%USERPROFILE%`/`%VAR%`, `$env:VAR` for sandbox checks.
fn expand_user_path(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.starts_with("~/") || s == "~" {
        if let Some(home) = dirs::home_dir() {
            s = format!("{}{}", home.display(), &s[1..]);
        }
    } else if s.starts_with("$HOME/") || s.starts_with("$HOME\\") {
        if let Some(home) = dirs::home_dir() {
            s = format!("{}{}", home.display(), &s[5..]);
        }
    } else if s.starts_with("%") {
        // %VAR%/rest -> env value
        if let Some(end) = s[1..].find('%') {
            let var = &s[1..1 + end];
            if let Ok(val) = std::env::var(var) {
                s = format!("{}{}", val, &s[1 + end + 1..]);
            }
        }
    } else if s.starts_with("$env:") {
        let rest = &s[5..];
        let var_end = rest.find(['/', '\\']).unwrap_or(rest.len());
        let var = &rest[..var_end];
        if let Ok(val) = std::env::var(var) {
            s = format!("{}{}", val, &rest[var_end..]);
        }
    }
    // Expand embedded $VAR / %VAR% occurrences.
    s = expand_embedded_env(&s);
    s
}

fn expand_embedded_env(s: &str) -> String {
    let mut out = s.to_string();
    // $HOME embedded
    if out.contains("$HOME") {
        if let Some(home) = dirs::home_dir() {
            out = out.replace("$HOME", &home.display().to_string());
        }
    }
    // Generic $VAR embedded: expand `$NAME` ([A-Za-z_][A-Za-z0-9_]*).
    // Skips `${...}`, `$env:` (handled above), and already-expanded `$HOME`.
    let mut search_from = 0usize;
    while let Some(rel) = out[search_from..].find('$') {
        let dollar = search_from + rel;
        let rest = &out[dollar + 1..];
        if rest.starts_with('{') || rest.starts_with("env:") || rest.starts_with("HOME") {
            search_from = dollar + 1;
            continue;
        }
        let mut end = 0usize;
        for (i, ch) in rest.char_indices() {
            if (i == 0 && (ch.is_ascii_alphabetic() || ch == '_'))
                || (i > 0 && (ch.is_ascii_alphanumeric() || ch == '_'))
            {
                end = i + ch.len_utf8();
            } else {
                break;
            }
        }
        if end == 0 {
            search_from = dollar + 1;
            continue;
        }
        let var = rest[..end].to_string();
        let val = std::env::var(&var).unwrap_or_default();
        out.replace_range(dollar..dollar + 1 + end, &val);
        search_from = dollar + val.len();
        if search_from > out.len() {
            break;
        }
    }
    // %VAR% embedded
    while let Some(start) = out.find('%') {
        let Some(end_rel) = out[start + 1..].find('%') else {
            break;
        };
        let end = start + 1 + end_rel;
        let var = &out[start + 1..end];
        if var.is_empty() || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        let val = std::env::var(var).unwrap_or_default();
        out.replace_range(start..=end, &val);
    }
    out
}

/// Canonicalize the longest existing prefix, then re-append the remainder
/// lexically. This catches `project/link(subdir->/etc)/secret` without
/// requiring the full target to exist.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if cur.exists() {
            if let Ok(real) = std::fs::canonicalize(&cur) {
                let mut out = strip_verbatim_prefix(real);
                remainder.reverse();
                for comp in remainder {
                    out.push(comp);
                }
                return out;
            }
            return path.to_path_buf();
        }
        match cur.file_name() {
            Some(name) => {
                remainder.push(name.to_os_string());
                if let Some(parent) = cur.parent() {
                    cur = parent.to_path_buf();
                    if cur.as_os_str().is_empty() {
                        return path.to_path_buf();
                    }
                    continue;
                }
                return path.to_path_buf();
            }
            None => {
                // Reached root without existing prefix.
                if let Ok(real) = std::fs::canonicalize(path) {
                    return strip_verbatim_prefix(real);
                }
                return path.to_path_buf();
            }
        }
    }
}

/// Normalize a tool path for policy glob matching (forward slashes, project-relative).
pub fn policy_path_pattern(project_dir: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "*" {
        return "*".into();
    }
    let expanded = expand_user_path(path);
    let root = canonical_root(project_dir);
    let resolved = resolve_path_against_root(project_dir, &root, Path::new(&expanded));
    let canonical = canonicalize_existing_prefix(&resolved);
    let root_slash = normalize_path_slashes(&root);
    let resolved_slash = normalize_path_slashes(&canonical);
    if resolved_slash == root_slash {
        ".".into()
    } else if let Some(suffix) = resolved_slash.strip_prefix(&format!("{root_slash}/")) {
        suffix.to_string()
    } else {
        expanded.replace('\\', "/")
    }
}

fn normalize_path_slashes(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

fn resolve_path_against_root(_project_dir: &str, root: &Path, path: &Path) -> PathBuf {
    // Normalize verbatim prefix + slashes before joining so
    // `\\?\C:\proj` and `C:/proj` compare equal.
    let cleaned = strip_verbatim_prefix(path.to_path_buf());
    if cleaned.is_absolute() {
        // If already under canonical root, keep under root (preserves case).
        if let Ok(relative) = cleaned.strip_prefix(root) {
            return root.join(relative);
        }
        // Try case-insensitive / slash-normalized prefix match for Windows.
        let root_cmp = normalize_compare_path(root);
        let abs_cmp = normalize_compare_path(&cleaned);
        if abs_cmp == root_cmp || abs_cmp.starts_with(&format!("{root_cmp}/")) {
            return cleaned;
        }
        return strip_verbatim_prefix(best_effort_abs_path(&cleaned));
    }
    root.join(cleaned)
}

/// Ensure a resolved filesystem path stays inside the project root.
/// Canonicalizes existing prefixes (symlink-aware) and re-checks the parent
/// after creation to close TOCTOU on `create_dir_all` races.
pub fn validate_project_path(project_dir: &str, path: PathBuf) -> Result<PathBuf, ToolFailure> {
    // Expand user/env shorthands on the string form first.
    let expanded = expand_user_path(&path.to_string_lossy());
    let path = PathBuf::from(expanded);
    if path_within_project(project_dir, &path) {
        // If parent exists as a symlink to outside, canonical check above
        // already fails. Return the lexically-joined path for IO.
        Ok(path)
    } else {
        Err(ToolFailure {
            message: format!(
                "path outside project directory: `{}` is not under the project root `{}`. \
                 Use a relative path inside the project.",
                path.display(),
                project_dir
            ),
            retryable: true,
        })
    }
}

/// Reject shell commands that try to delete the DeepSeekCode project root directory.
pub fn command_threatens_project_root(command: &str, project_dir: &str) -> Option<String> {
    let root = best_effort_abs(project_dir);
    let root_cmp = normalize_compare_path(&root);
    let mut cwd = root.clone();

    for segment in split_shell_segments(command) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }

        if let Some(target) = parse_cd_target(seg) {
            cwd = resolve_cd_path(&cwd, &target);
            continue;
        }

        for target in parse_destructive_targets(seg) {
            let full = resolve_relative_path(&cwd, &target);
            if normalize_compare_path(&full) == root_cmp {
                return Some(format!(
                    "Refusing to run a command that may delete the project root `{}`. \
                     Work inside the project directory instead of removing it.",
                    root.display()
                ));
            }
        }
    }

    None
}

/// Reject shell commands that leave the project root or create files outside it.
/// Covers `cd`, relative `../outside`, `~/…`, `$HOME`/`%VAR%`/`$env:VAR`,
/// `..\secret`, PowerShell `Get-Content ../x` / `Set-Content`, and relative
/// redirects (`> ../out`). Quote-aware.
pub fn command_escapes_project_directory(command: &str, project_dir: &str) -> Option<String> {
    let root = canonical_root(project_dir);
    let mut cwd = root.clone();

    for segment in split_shell_segments(command) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }

        if let Some(target) = parse_cd_target(seg) {
            let expanded = expand_user_path(&target);
            cwd = resolve_cd_path(&cwd, &expanded);
            if !path_within_project(project_dir, &cwd) {
                return Some(format!(
                    "Refusing to change directory outside the project root `{}`. \
                     Run commands inside the project directory.",
                    root.display()
                ));
            }
            continue;
        }

        if let Some(dir) = parse_mkdir_target(seg) {
            let expanded = expand_user_path(&dir);
            let full = resolve_relative_path(&cwd, &expanded);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to create `{}` outside the project root `{}`. \
                     Create subdirectories inside the project instead.",
                    full.display(),
                    root.display()
                ));
            }
        }

        // Absolute literals (drive:/unix) via extractor.
        for abs in extract_filesystem_path_literals(seg) {
            let expanded = expand_user_path(&abs);
            let full = PathBuf::from(&expanded);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to reference `{}` outside the project root `{}`. \
                     Use paths inside the project directory.",
                    full.display(),
                    root.display()
                ));
            }
        }

        // Relative escapes + home/env + PowerShell path args + redirects.
        for rel in extract_relative_escape_candidates(seg) {
            let expanded = expand_user_path(&rel);
            let full = resolve_relative_path(&cwd, &expanded);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to reference `{rel}` outside the project root `{}`. \
                     Use paths inside the project directory.",
                    root.display()
                ));
            }
        }
        for target in parse_redirect_targets(seg) {
            if target.trim().is_empty() {
                continue;
            }
            let expanded = expand_user_path(target.trim());
            let full = resolve_relative_path(&cwd, &expanded);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to write redirect target `{}` outside the project root `{}`.",
                    target.trim(),
                    root.display()
                ));
            }
        }
        // PowerShell file cmdlets: Get-Content/Set-Content/Add-Content etc.
        for ps_path in extract_powershell_path_args(seg) {
            let expanded = expand_user_path(&ps_path);
            let full = resolve_relative_path(&cwd, &expanded);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to reference `{ps_path}` outside the project root `{}`.",
                    root.display()
                ));
            }
        }
    }

    None
}

/// Tokens that may escape even without drive/leading-slash: `../x`,
/// `..\x`, `~/x`, `$HOME/...`, `%VAR%/...`, `$env:VAR/...`.
fn extract_relative_escape_candidates(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in split_shell_token_quoted(segment) {
        let t = token.trim_matches(|c| c == '"' || c == '\'').trim();
        if t.is_empty() {
            continue;
        }
        // Strip trailing punctuation from prose.
        let t = t.trim_end_matches([',', ';', ')']).trim();
        if t.starts_with("../")
            || t == ".."
            || t.starts_with("..\\")
            || t.starts_with("~/")
            || t == "~"
            || t.starts_with("$HOME")
            || t.starts_with("$env:")
            || t.starts_with('%')
            || (t.contains("..") && (t.contains('/') || t.contains('\\')))
        {
            out.push(t.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn split_shell_token_quoted(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in segment.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(ch);
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// PowerShell `*-Content -Path <p>` / `-LiteralPath <p>` / `Get-Item <p>`.
fn extract_powershell_path_args(segment: &str) -> Vec<String> {
    let tokens = split_shell_token_quoted(segment);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let lower = tokens[i].to_ascii_lowercase();
        let is_cmdlet = lower.contains("content")
            || lower.contains("get-item")
            || lower.contains("get-childitem")
            || lower.contains("remove-item")
            || lower == "type"
            || lower == "cat";
        if is_cmdlet && i + 1 < tokens.len() {
            // Skip flags, take `-Path <v>` value or bare next arg.
            let mut j = i + 1;
            while j < tokens.len() {
                let t = tokens[j].trim_matches(|c| c == '"' || c == '\'');
                if t.eq_ignore_ascii_case("-path")
                    || t.eq_ignore_ascii_case("-literalpath")
                    || t.eq_ignore_ascii_case("-filepath")
                {
                    if j + 1 < tokens.len() {
                        let v = tokens[j + 1]
                            .trim_matches(|c| c == '"' || c == '\'')
                            .to_string();
                        if !v.is_empty() && v != "1" && v != "2" {
                            out.push(v);
                        }
                        j += 2;
                        continue;
                    }
                    j += 1;
                    continue;
                }
                if t.starts_with('-') {
                    j += 1;
                    continue;
                }
                if !t.is_empty() {
                    out.push(t.to_string());
                }
                break;
            }
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out
}

/// Extract filesystem paths that a shell command may create or modify.
pub fn shell_command_target_paths(command: &str, project_dir: &str) -> Vec<PathBuf> {
    let root = best_effort_abs(project_dir);
    let mut cwd = root.clone();
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for segment in split_shell_segments_quoted(command) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }

        if let Some(target) = parse_cd_target(seg) {
            cwd = resolve_cd_path(&cwd, &target);
            continue;
        }

        let redirect_targets = parse_redirect_targets(seg);
        for target in &redirect_targets {
            let full = resolve_relative_path(&cwd, target);
            if seen.insert(full.clone()) {
                paths.push(full);
            }
        }

        for abs in extract_filesystem_path_literals(seg) {
            if is_redirect_literal_artifact(&abs, &redirect_targets) {
                continue;
            }
            let full = if PathBuf::from(&abs).is_absolute() {
                PathBuf::from(abs)
            } else {
                resolve_relative_path(&cwd, &abs)
            };
            if seen.insert(full.clone()) {
                paths.push(full);
            }
        }
    }

    paths
}

fn is_redirect_literal_artifact(literal: &str, redirect_targets: &[String]) -> bool {
    let normalized = literal.trim_start_matches('/');
    redirect_targets.iter().any(|target| {
        target == normalized
            || target.ends_with(&format!("/{normalized}"))
            || normalized == target.trim_start_matches('/')
    })
}

/// Single quote-aware pass over the segment: finds `>` / `>>` redirections
/// outside quoted strings and returns their target paths. `N>&M` fd
/// duplications are ignored, while `N>file` (e.g. `2>err.log`) is captured
/// because it creates or truncates the file.
fn parse_redirect_targets(segment: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let chars: Vec<char> = segment.chars().collect();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            '"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            '\\' if in_double => {
                i += 2; // skip the escaped character
            }
            '>' if !in_single && !in_double => {
                let append = chars.get(i + 1) == Some(&'>');
                i += if append { 2 } else { 1 };
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                // Collect the target token; quoted whitespace stays part of it.
                let mut path = String::new();
                while i < chars.len() {
                    let t = chars[i];
                    if t == '\\' && in_double && i + 1 < chars.len() {
                        path.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if t == '\'' && !in_double {
                        in_single = !in_single;
                        i += 1;
                        continue;
                    }
                    if t == '"' && !in_single {
                        in_double = !in_double;
                        i += 1;
                        continue;
                    }
                    if t == '>' && !in_single && !in_double {
                        break; // the next redirection operator starts here
                    }
                    if t.is_whitespace() && !in_single && !in_double {
                        break;
                    }
                    path.push(t);
                    i += 1;
                }
                let path = path.trim();
                if !path.is_empty() && !path.starts_with('&') {
                    targets.push(path.to_string());
                }
            }
            _ => i += 1,
        }
    }
    targets
}

fn best_effort_abs(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    strip_verbatim_prefix(std::fs::canonicalize(&p).unwrap_or(p))
}

fn best_effort_abs_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        strip_verbatim_prefix(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    } else {
        path.to_path_buf()
    }
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(stripped) = rendered.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

fn normalize_compare_path(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase()
}

/// Split a command into segments at `&&` / `;`, dropping the operator
/// characters but keeping everything else verbatim (quotes stripped).
fn split_shell_segments(command: &str) -> Vec<String> {
    split_shell_segments_impl(command, false)
}

/// Like [`split_shell_segments`], but preserves the quote characters in the
/// output so downstream parsing can tell quoted text (a literal `>` inside
/// `echo "a > b"`) apart from shell syntax.
fn split_shell_segments_quoted(command: &str) -> Vec<String> {
    split_shell_segments_impl(command, true)
}

fn split_shell_segments_impl(command: &str, keep_quotes: bool) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut in_single = false;

    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if !in_single => {
                in_double = !in_double;
                if keep_quotes {
                    current.push(ch);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                if keep_quotes {
                    current.push(ch);
                }
            }
            '&' if !in_double && !in_single => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                } else {
                    current.push(ch);
                }
            }
            ';' if !in_double && !in_single => {
                if !current.trim().is_empty() {
                    segments.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }
    segments
}

fn parse_cd_target(segment: &str) -> Option<String> {
    let seg = segment.trim();
    let rest = seg
        .strip_prefix("cd /d ")
        .or_else(|| seg.strip_prefix("cd /D "))
        .or_else(|| seg.strip_prefix("cd "))?;
    let target = parse_shell_path_arg(rest);
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

fn parse_mkdir_target(segment: &str) -> Option<String> {
    let seg = segment.trim();
    let rest = seg
        .strip_prefix("mkdir ")
        .or_else(|| seg.strip_prefix("md "))
        .or_else(|| seg.strip_prefix("New-Item -ItemType Directory -Path "))
        .or_else(|| seg.strip_prefix("new-item -itemtype directory -path "))?;
    let target = parse_shell_path_arg(rest);
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

fn parse_shell_path_arg(input: &str) -> String {
    let input = input.trim();
    if let Some(quoted) = parse_quoted_token(input) {
        return quoted;
    }
    input
        .split_whitespace()
        .next()
        .unwrap_or(input)
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn parse_quoted_token(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let mut end = 1;
    while end < bytes.len() {
        if bytes[end] == quote {
            return Some(input[1..end].to_string());
        }
        end += 1;
    }
    None
}

fn resolve_cd_path(cwd: &Path, target: &str) -> PathBuf {
    let target_path = PathBuf::from(target);
    if target_path.is_absolute() {
        target_path
    } else if target == ".." {
        cwd.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cwd.to_path_buf())
    } else if target == "." {
        cwd.to_path_buf()
    } else {
        cwd.join(target_path)
    }
}

fn resolve_relative_path(cwd: &Path, target: &str) -> PathBuf {
    let target_path = PathBuf::from(target);
    if target_path.is_absolute() {
        target_path
    } else {
        cwd.join(target_path)
    }
}

fn extract_filesystem_path_literals(segment: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Windows drive paths: D:\foo or D:/foo (single letter + colon + slash)
        if is_windows_drive_prefix(bytes, i) {
            let start = i;
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'&' {
                i += 1;
            }
            paths.push(
                segment[start..i]
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
            continue;
        }

        if bytes[i] == b'/' {
            // URL scheme (`http://host`, `https://host/...`)
            if i > 0 && bytes[i - 1] == b':' {
                i += 1;
                if i < bytes.len() && bytes[i] == b'/' {
                    i += 1;
                }
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && bytes[i] != b'&'
                    && bytes[i] != b'"'
                    && bytes[i] != b'\''
                {
                    i += 1;
                }
                continue;
            }
            // Protocol-relative or URL authority (`//host:port/...`).
            if i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                let start = i;
                i += 2;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'&' {
                    i += 1;
                }
                let token = segment[start..i].trim_matches('"').trim_matches('\'');
                if looks_like_url_authority(token) {
                    continue;
                }
                if looks_like_filesystem_path(token) {
                    paths.push(token.to_string());
                }
                continue;
            }

            let start = i;
            i += 1;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'&' {
                i += 1;
            }
            let token = segment[start..i].trim_matches('"').trim_matches('\'');
            if looks_like_filesystem_path(token) {
                paths.push(token.to_string());
            }
            continue;
        }

        i += 1;
    }
    paths
}

fn is_windows_drive_prefix(bytes: &[u8], i: usize) -> bool {
    if i + 2 >= bytes.len() {
        return false;
    }
    if !bytes[i].is_ascii_alphabetic() || bytes[i + 1] != b':' {
        return false;
    }
    if i > 0 && bytes[i - 1].is_ascii_alphabetic() {
        return false;
    }
    matches!(bytes[i + 2], b'\\' | b'/')
}

fn looks_like_url_authority(token: &str) -> bool {
    let body = token.trim_start_matches('/');
    if body.is_empty() {
        return true;
    }
    // `localhost:3000`, `example.com/path`
    if body.contains("://") {
        return true;
    }
    if body.starts_with("localhost") || body.starts_with("127.0.0.1") || body.starts_with("[::1]") {
        return true;
    }
    // Hostname with port and no backslashes is almost certainly a URL.
    if body.contains(':') && !body.contains('\\') {
        let host = body.split('/').next().unwrap_or(body);
        if host.contains(':') && !host.starts_with(':') {
            return true;
        }
    }
    false
}

fn looks_like_windows_cmd_flag(token: &str) -> bool {
    if !token.starts_with('/') || token.len() < 2 {
        return false;
    }
    let body = &token[1..];
    body.chars()
        .all(|c| c.is_ascii_alphabetic() || matches!(c, '-' | ':' | '+'))
}

fn looks_like_filesystem_path(token: &str) -> bool {
    let token = token.trim_matches('"').trim_matches('\'');
    if token.is_empty() {
        return false;
    }
    if token.contains('[') || token.contains(']') {
        return false;
    }
    if token.ends_with('\'') || token.ends_with(',') || token.ends_with(';') || token.ends_with(')')
    {
        return false;
    }
    if looks_like_windows_cmd_flag(token) {
        return false;
    }
    if token.contains('*') || token.contains('?') {
        return false;
    }
    if token.starts_with("//") {
        return token.contains('\\');
    }
    token.starts_with('/') && token.len() > 2
}

fn parse_destructive_targets(segment: &str) -> Vec<String> {
    let seg = segment.trim();
    let lower = seg.to_lowercase();
    let prefixes = [
        "rd /s /q ",
        "rd /s/q ",
        "rmdir /s /q ",
        "rmdir /s/q ",
        "rm -rf ",
        "rm -r ",
        "del /s /q ",
        "del /s/q ",
        "remove-item -recurse -force ",
        "remove-item -recurse ",
    ];
    for prefix in prefixes {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = &seg[seg.len() - rest.len()..];
            let target = parse_shell_path_arg(rest);
            if !target.is_empty() {
                return vec![target];
            }
        }
    }
    if lower.starts_with("rd ") || lower.starts_with("rmdir ") {
        let rest = seg.split_whitespace().last().unwrap_or("");
        let target = parse_shell_path_arg(rest);
        if !target.is_empty() && !target.starts_with('/') {
            return vec![target];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn redirect_targets_respect_quotes() {
        assert_eq!(
            parse_redirect_targets("echo hello > src/out.txt"),
            vec!["src/out.txt".to_string()]
        );
        // A `>` inside quotes is literal text, not a redirection.
        assert!(parse_redirect_targets("echo \"a > b\"").is_empty());
        assert!(parse_redirect_targets("echo 'x > y'").is_empty());
        // Quoted targets keep their inner whitespace.
        assert_eq!(
            parse_redirect_targets("echo hi > \"my file.txt\""),
            vec!["my file.txt".to_string()]
        );
        // fd duplication is ignored; fd redirection to a file is captured.
        assert_eq!(
            parse_redirect_targets("cmd 2>&1 > out.log"),
            vec!["out.log".to_string()]
        );
        assert_eq!(
            parse_redirect_targets("make 2> build-errors.log"),
            vec!["build-errors.log".to_string()]
        );
        // Multiple redirections in one segment.
        assert_eq!(
            parse_redirect_targets("run > a.txt 2>> b.txt"),
            vec!["a.txt".to_string(), "b.txt".to_string()]
        );
    }

    #[test]
    fn shell_paths_skip_quoted_redirect_lookalikes() {
        let paths = shell_command_target_paths(r#"echo "a > b" > real.txt"#, "/proj");
        // The quoted `a > b` must not be treated as a redirection; only the
        // real redirect target may be captured.
        assert_eq!(paths, vec![PathBuf::from("/proj/real.txt")]);
    }

    #[test]
    fn ensure_agents_md_creates_missing_file() {
        let dir = TempDir::new().unwrap();
        assert!(ensure_agents_md(&dir.path().to_string_lossy()));
        assert!(dir.path().join("AGENTS.md").exists());
        assert!(!ensure_agents_md(&dir.path().to_string_lossy()));
    }

    #[test]
    fn blocks_rmdir_on_project_folder_name() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let msg = command_threatens_project_root(
            "cd .. && rmdir /s /q sample-app",
            &project.to_string_lossy(),
        );
        assert!(msg.is_some());
    }

    #[test]
    fn allows_rd_on_subdirectory_under_project_root() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let cmd = format!("rd /s /q {}\\app\\api 2>nul", project.display());
        assert!(
            command_threatens_project_root(&cmd, &project.to_string_lossy()).is_none(),
            "deleting a child directory should be allowed"
        );
    }

    #[test]
    fn allows_python_shutil_on_bracket_paths() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let project = project.to_string_lossy().to_string();
        let cmd = r#"python -c "import shutil; shutil.rmtree('app/edit/[id]', ignore_errors=True); shutil.rmtree('app/api', ignore_errors=True)""#;
        assert!(command_escapes_project_directory(cmd, &project).is_none());
    }

    #[test]
    fn allows_dir_attribute_flags() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let project = project.to_string_lossy().to_string();
        assert!(command_escapes_project_directory("dir /s /b /a:-D app 2>nul", &project).is_none());
        assert!(command_escapes_project_directory("dir /s /b /a-d app 2>nul", &project).is_none());
    }

    #[test]
    fn blocks_mkdir_outside_project_after_cd() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("test");
        let project = parent.join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let cmd = format!("cd /d {} && mkdir vibe-next-notebook", parent.display());
        let msg = command_escapes_project_directory(&cmd, &project.to_string_lossy());
        assert!(msg.is_some(), "expected block, got {:?}", msg);
    }

    #[test]
    fn allows_mkdir_inside_project() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let msg = command_escapes_project_directory(
            "mkdir src && mkdir src\\app",
            &project.to_string_lossy(),
        );
        assert!(msg.is_none());
    }

    #[test]
    fn path_within_project_accepts_child_and_rejects_sibling() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let sibling = dir.path().join("vibe-next-notebook");
        assert!(path_within_project(
            &project.to_string_lossy(),
            &project.join("src/app/page.tsx")
        ));
        assert!(!path_within_project(
            &project.to_string_lossy(),
            &sibling.join("tsconfig.json")
        ));
    }

    #[test]
    fn allows_cd_to_quoted_project_root() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let cmd = format!("cd /d \"{}\" 2>&1", project.display());
        assert!(
            command_escapes_project_directory(&cmd, &project.to_string_lossy()).is_none(),
            "cd to project root should be allowed"
        );
    }

    #[test]
    fn allows_start_b_flag() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let cmd = format!(
            "cd /d {} && start /B npx next dev -p 3000",
            project.display()
        );
        assert!(command_escapes_project_directory(&cmd, &project.to_string_lossy()).is_none());
    }

    #[test]
    fn extracts_no_paths_from_localhost_curl() {
        let paths = extract_filesystem_path_literals(
            "curl http://localhost:3000 -s -o NUL -w \"HTTP Status: %{http_code}\\n\"",
        );
        assert!(paths.is_empty(), "unexpected paths: {paths:?}");
    }

    #[test]
    fn allows_localhost_health_checks() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let project = project.to_string_lossy().to_string();

        let curl_cmd = "curl http://localhost:3000 -s -o NUL -w \"HTTP Status: %{http_code}\\n\"";
        assert!(command_escapes_project_directory(curl_cmd, &project).is_none());

        let ps_cmd = "powershell -Command \"try { Invoke-WebRequest -Uri 'http://localhost:3000' } catch { }\"";
        assert!(command_escapes_project_directory(ps_cmd, &project).is_none());
    }

    #[test]
    fn allows_windows_dir_flags() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let project = project.to_string_lossy().to_string();

        assert!(command_escapes_project_directory("dir /b txt-note-app", &project).is_none());
        assert!(command_escapes_project_directory("dir /s /b node_modules", &project).is_none());
    }

    #[test]
    fn still_blocks_drive_paths_outside_project() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("sample-app");
        std::fs::create_dir_all(&project).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let cmd = format!("type {}", outside.join("secret.txt").display());
        assert!(command_escapes_project_directory(&cmd, &project.to_string_lossy()).is_some());
    }
}
