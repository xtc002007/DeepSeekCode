use std::collections::HashMap;
use std::path::Path;

/// Resolve the shell executable to use for subprocesses.
pub fn resolve_shell(configured: Option<&str>) -> String {
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        configured = ?configured,
        "resolve_shell requested"
    );
    if let Some(shell) = configured.filter(|s| !s.trim().is_empty()) {
        return shell.to_string();
    }
    if cfg!(windows) {
        return std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// Parse null-delimited `env -0` output into key/value pairs.
pub fn parse_shell_env_output(bytes: &[u8]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for line in bytes.split(|b| *b == 0) {
        if line.is_empty() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        if !key.is_empty() {
            env.insert(key.to_string(), value.to_string());
        }
    }
    env
}

fn is_nushell(shell: &str) -> bool {
    Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("nu") || n.eq_ignore_ascii_case("nu.exe"))
        .unwrap_or(false)
}

/// Parse Windows `set` output (`KEY=value` per line).
pub fn parse_windows_set_output(text: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.is_empty() {
            env.insert(key.to_string(), value.to_string());
        }
    }
    env
}

fn probe_windows_cmd() -> Option<HashMap<String, String>> {
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let mut cmd = std::process::Command::new(&comspec);
    crate::process::hide_console(&mut cmd);
    let output = cmd
        .args(["/C", "set"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let env = parse_windows_set_output(&text);
    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

fn probe_shell(shell: &str, mode: &str) -> Option<HashMap<String, String>> {
    if cfg!(windows) {
        return probe_windows_cmd();
    }
    if is_nushell(shell) {
        return None;
    }

    let mut cmd = std::process::Command::new(shell);
    crate::process::hide_console(&mut cmd);
    let output = cmd
        .args([mode, "-c", "env -0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let env = parse_shell_env_output(&output.stdout);
    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

/// Capture environment variables for shell subprocesses, optionally probing login shell.
pub fn capture_shell_env(configured_shell: Option<&str>) -> HashMap<String, String> {
    let start = std::time::Instant::now();
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        shell = ?configured_shell,
        "capture_shell_env started"
    );
    let parent: HashMap<String, String> = std::env::vars()
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect();

    let shell = resolve_shell(configured_shell);
    let probed = if cfg!(windows) {
        probe_windows_cmd()
    } else {
        probe_shell(&shell, "-il").or_else(|| probe_shell(&shell, "-l"))
    };
    let merged = merge_shell_env(probed.as_ref(), &parent);
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        shell = %shell,
        vars = merged.len(),
        probed = probed.is_some(),
        duration_ms = start.elapsed().as_millis() as u64,
        "capture_shell_env completed"
    );
    merged
}

/// Merge probed shell env under explicit parent overrides.
pub fn merge_shell_env(
    probed: Option<&HashMap<String, String>>,
    parent: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = probed.cloned().unwrap_or_default();
    merged.extend(parent.clone());
    merged
}

/// Apply captured env to a tokio Command.
pub fn apply_to_command(cmd: &mut tokio::process::Command, env: &HashMap<String, String>) {
    for (key, value) in env {
        cmd.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_null_delimited_env() {
        let env = parse_shell_env_output(b"PATH=/usr/bin\0FOO=bar=baz\0\0");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar=baz"));
    }

    #[test]
    fn capture_includes_path() {
        let env = capture_shell_env(None);
        assert!(env.contains_key("PATH") || env.contains_key("Path"));
    }

    #[test]
    fn merge_prefers_parent_overrides() {
        let mut probed = HashMap::new();
        probed.insert("FOO".into(), "from_shell".into());
        let mut parent = HashMap::new();
        parent.insert("FOO".into(), "from_parent".into());
        let merged = merge_shell_env(Some(&probed), &parent);
        assert_eq!(merged.get("FOO").map(String::as_str), Some("from_parent"));
    }

    #[test]
    fn parse_windows_set_lines() {
        let env = parse_windows_set_output("Path=C:\\Windows\r\nFOO=bar\r\n");
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert!(env.contains_key("Path"));
    }

    #[test]
    fn windows_capture_merges_parent() {
        if !cfg!(windows) {
            return;
        }
        std::env::set_var("DEEPSEEKCODE_SHELL_ENV_TEST", "parent-value");
        let env = capture_shell_env(None);
        assert!(
            env.contains_key("PATH") || env.contains_key("Path"),
            "expected Path from cmd probe or parent"
        );
        assert_eq!(
            env.get("DEEPSEEKCODE_SHELL_ENV_TEST").map(String::as_str),
            Some("parent-value")
        );
        std::env::remove_var("DEEPSEEKCODE_SHELL_ENV_TEST");
    }
}
