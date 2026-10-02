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

/// Prepend every `node_modules/.bin` from `dir` up to the filesystem root to
/// `PATH`, so locally installed CLIs are callable.
///
/// npm/pnpm/yarn install project binaries into `node_modules/.bin` and never
/// register them globally. Without this, `typescript-language-server`,
/// `eslint`, `vite` etc. are "program not found" even though they are
/// installed. Agents like Cursor / Claude Code / Copilot all do this.
///
/// Nearest directory wins, so a monorepo package's own binaries take priority
/// over the workspace root's.
pub fn path_with_local_bins(dir: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = Some(dir);
    while let Some(current) = cursor {
        let bin = current.join("node_modules").join(".bin");
        if bin.is_dir() {
            parts.push(bin.to_string_lossy().to_string());
        }
        cursor = current.parent();
    }
    if parts.is_empty() {
        return existing;
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut out = parts.join(sep);
    out.push_str(sep);
    out.push_str(&existing);
    out
}

/// Locate an executable, preferring the `node_modules/.bin` shims.
///
/// Windows note: npm/pnpm write `foo.cmd` / `foo.ps1` shims, **never**
/// `foo.exe`. `std::process::Command` only resolves `.exe`, so spawning the
/// bare name fails even with the right PATH. This returns the `.cmd` path so
/// the caller can run it through `cmd.exe /C`.
pub fn resolve_executable(command: &str, working_dir: &Path) -> Option<std::path::PathBuf> {
    let raw = Path::new(command);
    if raw.components().count() > 1 && raw.is_file() {
        return Some(raw.to_path_buf());
    }
    let name = raw.file_name()?.to_string_lossy().to_string();
    // Candidate order is platform-dependent and the order matters:
    // on Windows npm/pnpm write `foo` (a POSIX sh script), `foo.cmd` (batch)
    // and `foo.ps1`. Preferring the extension-less file there resolves to a
    // shell script Windows cannot execute, so `.cmd` must win.
    let candidates: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.cmd"),
            format!("{name}.bat"),
            format!("{name}.exe"),
            name.clone(),
        ]
    } else {
        vec![name.clone(), format!("{name}.exe")]
    };
    let mut cursor = Some(working_dir);
    while let Some(current) = cursor {
        let bin = current.join("node_modules").join(".bin");
        for candidate in &candidates {
            let path = bin.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
        cursor = current.parent();
    }
    None
}

/// Resolve a spawned program into `(program, args)` that can actually run.
///
/// Two Windows/Node realities are handled:
/// 1. npm/pnpm write `foo.cmd` shims into `node_modules/.bin`, never `foo.exe`.
///    `std::process::Command` only resolves `.exe`, so the bare name fails even
///    with a correct PATH — a `.cmd`/`.bat` must be run via `cmd.exe /C`.
/// 2. Project binaries are not on the system PATH, so look them up in
///    `node_modules/.bin` from `working_dir` upwards.
///
/// The returned args keep the caller's ordering: for a `.cmd` shim the shim
/// path becomes the argument to `cmd.exe /C`, followed by the caller's own
/// arguments.
pub fn resolve_program(
    program: &str,
    args: &[String],
    working_dir: &Path,
) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    let resolved = resolve_executable(program, working_dir);
    let is_script = resolved
        .as_ref()
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| cfg!(windows) && (e == "cmd" || e == "bat"));

    if is_script {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let mut argv: Vec<std::ffi::OsString> = vec![
            "/C".into(),
            resolved.expect("checked above").into_os_string(),
        ];
        argv.extend(args.iter().map(std::ffi::OsString::from));
        (comspec.into(), argv)
    } else {
        (
            resolved
                .map(std::ffi::OsString::from)
                .unwrap_or_else(|| program.into()),
            args.iter().map(std::ffi::OsString::from).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `typescript-language-server` was installed in the project's
    /// `node_modules/.bin` yet reported "program not found": project binaries
    /// are never on the system PATH, so the lookup must walk up from the
    /// working directory.
    #[test]
    fn resolve_executable_finds_project_local_bin() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shim = bin.join("my-lsp");
        std::fs::write(&shim, "#!/bin/sh\n").unwrap();
        #[cfg(windows)]
        std::fs::write(bin.join("my-lsp.cmd"), "@echo off\r\n").unwrap();

        let nested = dir.path().join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();

        let found = resolve_executable("my-lsp", &nested).expect("must resolve");
        assert_eq!(
            found.file_name().unwrap().to_string_lossy().split('.').next().unwrap(),
            "my-lsp"
        );
        assert!(resolve_executable("definitely-not-installed", &nested).is_none());
    }

    /// Windows npm/pnpm shims are `.cmd`, which `Command` cannot spawn by
    /// name — they must be routed through `cmd.exe /C`.
    #[cfg(windows)]
    #[test]
    fn resolve_program_wraps_cmd_shims() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tsls.cmd"), "@echo off\r\n").unwrap();

        let (program, args) = resolve_program("tsls", &["--stdio".into()], dir.path());
        let program = program.to_string_lossy().to_lowercase();
        assert!(
            program.contains("cmd"),
            "expected cmd.exe, got {program}"
        );
        assert_eq!(args[0].to_string_lossy(), "/C");
        assert!(args[1].to_string_lossy().ends_with("tsls.cmd"), "{args:?}");
        assert_eq!(args[2].to_string_lossy(), "--stdio", "caller args must be preserved");
    }

    /// An unresolvable name falls back to the literal program, unchanged.
    #[test]
    fn resolve_program_keeps_plain_executables() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = resolve_program("my-real-binary.exe", &["-x".into()], dir.path());
        assert_eq!(program.to_string_lossy(), "my-real-binary.exe");
        assert_eq!(args[0].to_string_lossy(), "-x");
    }

    #[test]
    fn path_with_local_bins_prepends_nearest_bin_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root_bin = dir.path().join("node_modules").join(".bin");
        std::fs::create_dir_all(&root_bin).unwrap();
        let pkg = dir.path().join("packages").join("app");
        let pkg_bin = pkg.join("node_modules").join(".bin");
        std::fs::create_dir_all(&pkg_bin).unwrap();

        let path = path_with_local_bins(&pkg);
        let sep = if cfg!(windows) { ';' } else { ':' };
        let parts: Vec<&str> = path.split(sep).collect();
        // Nearest wins: the package's own bin dir must come first.
        assert!(parts[0].contains("app"), "nearest bin dir not first: {parts:?}");
        // Both levels are included, root one after the package one.
        assert_eq!(
            parts.iter().filter(|p| p.contains("node_modules")).count(),
            2
        );
        // The original PATH is preserved as the suffix.
        let original = std::env::var("PATH").unwrap_or_default();
        if !original.is_empty() {
            assert!(
                path.ends_with(&original),
                "original PATH must be preserved at the end"
            );
        }
    }

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
