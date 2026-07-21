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
pub fn path_within_project(project_dir: &str, path: &Path) -> bool {
    let root = best_effort_abs(project_dir);
    let resolved = resolve_path_against_root(project_dir, &root, path);
    let root_cmp = normalize_compare_path(&root);
    let target_cmp = normalize_compare_path(&resolved);
    target_cmp == root_cmp || target_cmp.starts_with(&format!("{root_cmp}/"))
}

/// Normalize a tool path for policy glob matching (forward slashes, project-relative).
pub fn policy_path_pattern(project_dir: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "*" {
        return "*".into();
    }
    let root = best_effort_abs(project_dir);
    let resolved = resolve_path_against_root(project_dir, &root, Path::new(path));
    let root_slash = normalize_path_slashes(&root);
    let resolved_slash = normalize_path_slashes(&resolved);
    if resolved_slash == root_slash {
        ".".into()
    } else if let Some(suffix) = resolved_slash.strip_prefix(&format!("{root_slash}/")) {
        suffix.to_string()
    } else {
        path.replace('\\', "/")
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

fn resolve_path_against_root(project_dir: &str, root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        if let Ok(relative) = path.strip_prefix(project_dir) {
            return root.join(relative);
        }
        if let Ok(relative) = path.strip_prefix(root) {
            return root.join(relative);
        }
        best_effort_abs_path(path)
    } else {
        root.join(path)
    }
}

/// Ensure a resolved filesystem path stays inside the project root.
pub fn validate_project_path(project_dir: &str, path: PathBuf) -> Result<PathBuf, ToolFailure> {
    if path_within_project(project_dir, &path) {
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
pub fn command_escapes_project_directory(command: &str, project_dir: &str) -> Option<String> {
    let root = best_effort_abs(project_dir);
    let mut cwd = root.clone();

    for segment in split_shell_segments(command) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }

        if let Some(target) = parse_cd_target(seg) {
            cwd = resolve_cd_path(&cwd, &target);
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
            let full = resolve_relative_path(&cwd, &dir);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to create `{}` outside the project root `{}`. \
                     Create subdirectories inside the project instead.",
                    full.display(),
                    root.display()
                ));
            }
        }

        for abs in extract_filesystem_path_literals(seg) {
            let full = PathBuf::from(&abs);
            if !path_within_project(project_dir, &full) {
                return Some(format!(
                    "Refusing to reference `{}` outside the project root `{}`. \
                     Use paths inside the project directory.",
                    full.display(),
                    root.display()
                ));
            }
        }
    }

    None
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

fn split_shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut in_single = false;

    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
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
