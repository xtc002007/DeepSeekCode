use async_trait::async_trait;
use code_plus_plus_schema::tool::{TodoStoreOps, ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::shell_env;

use super::agent_pty::{global_agent_pty_registry, strip_ansi_escapes};
use super::display::compute_text_diff_stats;
use super::registry::Tool;
use super::services::{
    current_services, emit_todo_updated, ensure_not_cancelled, run_blocking_if_cancelled,
};

macro_rules! simple_tool {
    ($name:ident, $tool_name:expr, $desc:expr, $schema:expr) => {
        pub struct $name;
    };
}

simple_tool!(
    ReadTool,
    "read",
    "Read a file from the filesystem",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path to read" },
            "offset": { "type": "integer", "description": "Start line (1-based)" },
            "limit": { "type": "integer", "description": "Max lines to read" }
        },
        "required": ["path"]
    })
);

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read a file from the filesystem"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read" },
                "offset": { "type": "integer", "description": "Start line (1-based)" },
                "limit": { "type": "integer", "description": "Max lines to read" }
            },
            "required": ["path"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "read",
            path = %input["path"].as_str().unwrap_or(""),
            "read started"
        );
        let path = resolve_path(ctx, input["path"].as_str().unwrap_or(""))?;
        if !path.exists() {
            tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %ctx.session_id,
                tool = "read",
                path = %path.display(),
                "read failed: path not found"
            );
            return Err(ToolFailure {
                message: format!("path not found: {}", path.display()),
                retryable: false,
            });
        }
        if path.is_dir() {
            return Err(ToolFailure {
                message: format!(
                    "{} is a directory, not a file. Use the glob tool to list files, or bash with dir/ls.",
                    path.display()
                ),
                retryable: false,
            });
        }
        // Bound file size (8MiB) and stream lines instead of loading
        // unbounded files into memory (P0-4 OOM guard).
        const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;
        let meta = std::fs::metadata(&path).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        if meta.len() > MAX_READ_BYTES {
            return Err(ToolFailure {
                message: format!(
                    "file too large ({} bytes, max 8MiB); use offset/limit paging",
                    meta.len()
                ),
                retryable: false,
            });
        }
        let content = std::fs::read_to_string(&path).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = (input["limit"].as_u64().unwrap_or(2000) as usize).min(5000);
        let lines: Vec<&str> = content.lines().skip(offset - 1).take(limit).collect();
        let line_start = offset as u32;
        let line_end = if lines.is_empty() {
            line_start
        } else {
            line_start + lines.len() as u32 - 1
        };
        Ok(json!({
            "content": lines.join("\n"),
            "path": path.display().to_string(),
            "line_start": line_start,
            "line_end": line_end
        }))
    }
}

simple_tool!(
    WriteTool,
    "write",
    "Write content to a file",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "content": { "type": "string" }
        },
        "required": ["path", "content"]
    })
);

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Write content to a file"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "write",
            path = %input["path"].as_str().unwrap_or(""),
            content_bytes = input["content"].as_str().map(|s| s.len()).unwrap_or(0),
            "write started"
        );
        let path = resolve_path(ctx, input["path"].as_str().unwrap_or(""))?;
        let content = input["content"].as_str().unwrap_or("").to_string();
        let path_display = path.clone();
        run_blocking_if_cancelled(move || {
            ensure_not_cancelled()?;
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolFailure {
                    message: e.to_string(),
                    retryable: false,
                })?;
            }
            ensure_not_cancelled()?;
            std::fs::write(&path, &content).map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            let diff_stats = compute_text_diff_stats(&old, &content);
            Ok(json!({
                "path": path_display.display().to_string(),
                "written": path_display.display().to_string(),
                "bytes": content.len(),
                "diff_stats": diff_stats,
            }))
        })
        .await
    }
}

simple_tool!(
    EditTool,
    "edit",
    "Replace a unique string in a file",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "old_string": { "type": "string" },
            "new_string": { "type": "string" }
        },
        "required": ["path", "old_string", "new_string"]
    })
);

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Replace a unique string in a file"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "edit",
            path = %input["path"].as_str().unwrap_or(""),
            "edit started"
        );
        let path = resolve_path(ctx, input["path"].as_str().unwrap_or(""))?;
        let old_string = input["old_string"].as_str().unwrap_or("").to_string();
        let new_string = input["new_string"].as_str().unwrap_or("").to_string();
        let path_display = path.clone();
        run_blocking_if_cancelled(move || {
            ensure_not_cancelled()?;
            let content = std::fs::read_to_string(&path).map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            let count = content.matches(old_string.as_str()).count();
            if count != 1 {
                return Err(ToolFailure {
                    message: format!("expected 1 match, found {count}"),
                    retryable: true,
                });
            }
            let updated = content.replacen(old_string.as_str(), new_string.as_str(), 1);
            ensure_not_cancelled()?;
            std::fs::write(&path, &updated).map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            let diff_stats = compute_text_diff_stats(&content, &updated);
            Ok(json!({
                "path": path_display.display().to_string(),
                "replaced": true,
                "diff_stats": diff_stats,
            }))
        })
        .await
    }
}

simple_tool!(
    GrepTool,
    "grep",
    "Search for a pattern in files",
    json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string" },
            "path": { "type": "string" }
        },
        "required": ["pattern"]
    })
);

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search for a pattern in files"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" }
            },
            "required": ["pattern"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "grep",
            pattern_len = input["pattern"].as_str().map(|s| s.len()).unwrap_or(0),
            "grep started"
        );
        let pattern = input["pattern"].as_str().unwrap_or("");
        let search_path = input["path"]
            .as_str()
            .map(|p| resolve_path(ctx, p))
            .transpose()?
            .unwrap_or_else(|| PathBuf::from(&ctx.working_directory));

        let mut matches = Vec::new();
        let walker = ignore::WalkBuilder::new(&search_path)
            .hidden(false)
            .git_ignore(true)
            .build();
        for entry in walker.flatten().take(500) {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                // Skip huge/binary files to bound memory (P0-4).
                if entry
                    .metadata()
                    .map(|m| m.len() > 1024 * 1024)
                    .unwrap_or(true)
                {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for (i, line) in content.lines().enumerate() {
                        if line.contains(pattern) {
                            matches.push(json!({
                                "path": entry.path().display().to_string(),
                                "line": i + 1,
                                "text": line,
                            }));
                        }
                    }
                }
            }
        }
        Ok(json!({ "matches": matches }))
    }
}

simple_tool!(
    GlobTool,
    "glob",
    "Find files matching a glob pattern",
    json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string" }
        },
        "required": ["pattern"]
    })
);

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files matching a glob pattern"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" }
            },
            "required": ["pattern"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "glob",
            pattern = %input["pattern"].as_str().unwrap_or("*"),
            "glob started"
        );
        let pattern = input["pattern"].as_str().unwrap_or("*");
        let full_pattern = format!(
            "{}/{}",
            ctx.working_directory.trim_end_matches('/'),
            pattern
        );
        let files: Vec<String> = glob::glob(&full_pattern)
            .map_err(|e| {
                tracing::error!(
                    target: code_plus_plus_telemetry::TOOL,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    session_id = %ctx.session_id,
                    tool = "glob",
                    error = %e,
                    "glob failed"
                );
                ToolFailure {
                    message: e.to_string(),
                    retryable: false,
                }
            })?
            .filter_map(|p| p.ok())
            .map(|p| p.display().to_string())
            .take(200)
            .collect();
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "glob",
            files = files.len(),
            "glob succeeded"
        );
        Ok(json!({ "files": files }))
    }
}

pub struct PowershellTool {
    bash: BashTool,
}

impl PowershellTool {
    pub fn new(shell: Option<String>, env: HashMap<String, String>) -> Self {
        Self {
            bash: BashTool::new(shell, env),
        }
    }

    pub fn with_pty(mut self, pty: bool) -> Self {
        self.bash = self.bash.with_pty(pty);
        self
    }
}

#[async_trait]
impl Tool for PowershellTool {
    fn name(&self) -> &str {
        "powershell"
    }
    fn description(&self) -> &str {
        "Execute a PowerShell command (Windows). Prefer `bash` for cross-platform shell commands."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout_ms": { "type": "integer" }
            },
            "required": ["command"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "powershell",
            command_len = input["command"].as_str().map(|s| s.len()).unwrap_or(0),
            "powershell started"
        );
        let command = input["command"].as_str().unwrap_or("");
        if command.trim().is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %ctx.session_id,
                tool = "powershell",
                "powershell failed: empty command"
            );
            return Err(ToolFailure {
                message: "command must not be empty".into(),
                retryable: false,
            });
        }
        let executable = if cfg!(windows) { "powershell" } else { "pwsh" };
        let wrapped = format!("{executable} -NoProfile -Command {command}");
        let mut bash_input = input;
        bash_input["command"] = json!(wrapped);
        self.bash.execute(bash_input, ctx).await
    }
}

pub struct BashTool {
    shell: Option<String>,
    env: HashMap<String, String>,
    /// Run the command under a real PTY (the agent-visible terminal) instead
    /// of pipes. Default `false`; the registry enables it from
    /// `CODE_PLUS_PLUS_BASH_PTY=1`, tests opt in per instance (never via env,
    /// which would race parallel tests).
    pty: bool,
}

impl BashTool {
    pub fn new(shell: Option<String>, env: HashMap<String, String>) -> Self {
        Self {
            shell,
            env,
            pty: false,
        }
    }

    pub fn with_pty(mut self, pty: bool) -> Self {
        self.pty = pty;
        self
    }
}

/// Build `(program, argv)` for a shell command, shared by the pipe and PTY
/// backends so both execute byte-identical command lines (including the
/// Windows `chcp 65001` prefix and the PowerShell non-interactive flags).
fn build_shell_argv(shell: Option<&str>, command: &str) -> (String, Vec<String>) {
    if let Some(shell) = shell {
        if is_powershell(shell) {
            (
                shell.to_string(),
                vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            )
        } else if is_posix_style_shell(shell) {
            (shell.to_string(), vec!["-lc".to_string(), command.to_string()])
        } else if cfg!(windows) {
            // cmd.exe family: force UTF-8 here too, otherwise configuring
            // a shell silently dropped the `chcp 65001` the default path
            // applies and non-ASCII output turned into mojibake.
            (
                shell.to_string(),
                vec![
                    "/C".to_string(),
                    format!("chcp 65001 >nul && {command}"),
                ],
            )
        } else {
            (shell.to_string(), vec!["-lc".to_string(), command.to_string()])
        }
    } else if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), format!("chcp 65001 >nul && {command}")],
        )
    } else {
        (
            "bash".to_string(),
            vec!["-lc".to_string(), command.to_string()],
        )
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Execute a shell command. On Windows commands run via cmd.exe (/c); use cmd syntax (e.g. dir, findstr, &&). On Unix, runs via bash -lc. \
         IMPORTANT: the command is killed after `timeout_ms` (default 60000, max 600000). Build, install and test \
         commands routinely exceed 60s, so ALWAYS pass an explicit larger `timeout_ms` for them \
         (e.g. pnpm install / typecheck / tsc / jest / cargo build / cargo test →timeout_ms 600000) \
         instead of retrying and reporting the tool as unusable."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Kill the command after this many milliseconds. Default 60000, clamped to 1000..600000. Pass a large value (e.g. 600000) for install/build/test commands."
                }
            },
            "required": ["command"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "bash",
            command_len = input["command"].as_str().map(|s| s.len()).unwrap_or(0),
            "bash started"
        );
        let command = input["command"].as_str().unwrap_or("");
        if command.trim().is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %ctx.session_id,
                tool = "bash",
                "bash failed: empty command"
            );
            return Err(ToolFailure {
                message: "command must not be empty".into(),
                retryable: false,
            });
        }
        if let Some(reason) =
            crate::project_context::command_threatens_project_root(command, &ctx.working_directory)
        {
            return Err(ToolFailure {
                message: reason,
                retryable: false,
            });
        }
        if let Some(reason) = crate::project_context::command_escapes_project_directory(
            command,
            &ctx.working_directory,
        ) {
            return Err(ToolFailure {
                message: reason,
                retryable: false,
            });
        }
        let command = prepare_background_bash_command(command);
        let timeout_ms = input["timeout_ms"]
            .as_u64()
            .unwrap_or(60_000)
            .clamp(1_000, 600_000);

        // PTY backend first: the terminal IS the execution site (single run,
        // visible + interactive). Falls back to pipes when ConPTY is
        // unavailable so a degraded host never loses command execution.
        if self.pty {
            match run_command_in_pty(self, &command, timeout_ms, ctx).await {
                PtyRunOutcome::Ok(value) => {
                    let _ = crate::project_context::ensure_agents_md(&ctx.working_directory);
                    return Ok(value);
                }
                PtyRunOutcome::Err(failure) => return Err(failure),
                PtyRunOutcome::Unavailable(_) => {}
            }
        }
        let (program, argv) = build_shell_argv(self.shell.as_deref(), &command);
        let mut cmd = Command::new(&program);
        cmd.args(&argv);
        cmd.current_dir(&ctx.working_directory);
        shell_env::apply_to_command(&mut cmd, &self.env);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        crate::process::hide_console_async(&mut cmd);

        // Shared so the timeout branch can tear down the whole process tree:
        // `kill_on_drop` only reaps the direct child.
        let spawned_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let output = match timeout(
            Duration::from_millis(timeout_ms),
            run_command_streaming(cmd, ctx, Arc::clone(&spawned_pid)),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                // The direct child is killed by `kill_on_drop`, but its
                // grandchildren (`node`, `pnpm`, `cargo`, a spawned server)
                // survive on Windows and keep holding files, ports and the
                // console —which is what made every LATER command in the
                // session time out too. Tear down the whole tree first.
                crate::process::kill_process_tree(Some(
                    spawned_pid.load(std::sync::atomic::Ordering::SeqCst),
                ));
                return Err(ToolFailure {
                    message: format!(
                        "command timed out after {timeout_ms}ms and was killed \
                         (its process tree too). Re-run with a larger `timeout_ms` \
                         (max 600000) if the command legitimately needs longer, \
                         or split it into smaller steps."
                    ),
                    retryable: true,
                });
            }
        };

        let _ = crate::project_context::ensure_agents_md(&ctx.working_directory);
        Ok(output)
    }
}

/// Reader over a child's output stream that yields decoded lines and NEVER
/// fails on non-UTF-8 bytes.
///
/// `BufRead::lines()` is unusable here: it returns `Err` the moment a line is
/// not valid UTF-8, and the caller then dropped the whole reader — so a single
/// Chinese character in `dir` output silently discarded every remaining byte,
/// including plain ASCII that followed it.
///
/// Windows consoles emit the active OEM/ANSI code page (GBK/CP936 on zh-CN)
/// even after `chcp 65001`, because the code page is applied to the console
/// buffer while redirected output keeps the program's own encoding. We
/// therefore decode as UTF-8 first and fall back to the system code page.
struct OutputLines<R> {
    reader: tokio::io::BufReader<R>,
    scratch: Vec<u8>,
    /// Bytes buffered so far; a line may straddle two reads.
    pending: Vec<u8>,
    done: bool,
}

impl<R: tokio::io::AsyncRead + Unpin> OutputLines<R> {
    fn new(inner: R) -> Self {
        Self {
            reader: tokio::io::BufReader::new(inner),
            scratch: vec![0u8; 16 * 1024],
            pending: Vec::new(),
            done: false,
        }
    }

    /// Next decoded line, or `None` at EOF. Only genuine I/O errors are
    /// reported; undecodable bytes are replaced, never fatal.
    async fn next_decoded_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
                // Take the line WITHOUT its terminator; the caller re-joins
                // lines with `\n`, and keeping it here produced `\r\n\n`.
                let line: Vec<u8> = self.pending.drain(..pos).collect();
                self.pending.remove(0);
                return Ok(Some(decode_console_bytes(trim_cr(&line))));
            }
            if self.done {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                let line = std::mem::take(&mut self.pending);
                return Ok(Some(decode_console_bytes(trim_cr(&line))));
            }
            let n = self.reader.read(&mut self.scratch).await?;
            if n == 0 {
                self.done = true;
            } else {
                self.pending.extend_from_slice(&self.scratch[..n]);
            }
        }
    }
}

fn trim_cr(line: &[u8]) -> &[u8] {
    match line.split_last() {
        Some((b'\r', rest)) => rest,
        _ => line,
    }
}

/// Decode console output, tolerating a legacy code page.
fn decode_console_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    // Not UTF-8. Fall back to the machine's legacy code page (GBK on zh-CN,
    // CP1252 elsewhere) rather than dropping the line.
    let (text, _, _) = console_encoding().decode(bytes);
    text.into_owned()
}

/// Map a Windows code page number to an `encoding_rs` encoding.
///
/// `Encoding::for_label("cp936")` returns `None` — it only accepts WHATWG
/// labels such as `gbk`/`gb18030`, so silently falling back to Windows-1252
/// turned every Chinese character into mojibake.
#[cfg(windows)]
fn encoding_for_code_page(cp: u32) -> &'static encoding_rs::Encoding {
    const CP_UTF8: u32 = 65001;
    if cp == CP_UTF8 {
        return encoding_rs::UTF_8;
    }
    // East Asian code pages all decode as GBK/GB18030 in encoding_rs.
    let label: &[u8] = match cp {
        936 | 950 | 54936 => b"gbk", // GBK, Big5, GB18030
        932 => b"shift_jis",         // Japanese
        949 => b"euc-kr",            // Korean
        1252 => b"windows-1252",
        1251 => b"windows-1251",
        1250 => b"windows-1250",
        _ => b"windows-1252",
    };
    encoding_rs::Encoding::for_label(label).unwrap_or(encoding_rs::WINDOWS_1252)
}

#[cfg(windows)]
fn console_encoding() -> &'static encoding_rs::Encoding {
    // cmd.exe emits redirected output in the machine's OEM code page (936 /
    // GBK on zh-CN).
    encoding_for_code_page(unsafe { windows_sys::Win32::Globalization::GetOEMCP() })
}

#[cfg(not(windows))]
fn console_encoding() -> &'static encoding_rs::Encoding {
    encoding_rs::UTF_8
}

async fn run_command_streaming(
    mut cmd: Command,
    ctx: &ToolContext,
    spawned_pid: Arc<std::sync::atomic::AtomicU32>,
) -> std::result::Result<Value, ToolFailure> {
    cmd.kill_on_drop(true);
    crate::process::prepare_tracked_command_async(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| ToolFailure {
        message: e.to_string(),
        retryable: true,
    })?;
    if let Some(pid) = child.id() {
        spawned_pid.store(pid, std::sync::atomic::Ordering::SeqCst);
    }
    // RAII: unregisters on every exit path —including future cancellation by
    // the timeout wrapper, which would otherwise leave a stale PID behind.
    let _pid_guard = child
        .id()
        .map(|pid| crate::process::SessionChildPidGuard::register(ctx.session_id, pid));
    let services = current_services();
    let cancel_token = services.as_ref().and_then(|s| s.cancel.clone());

    // Bounded capture (1MiB per stream): truncation happens DURING capture,
    // not after, so a runaway command cannot OOM before ManagedToolOutput
    // clamps (P0-4).
    const MAX_STREAM_BYTES: usize = 1024 * 1024;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    // Read stdout and stderr CONCURRENTLY: draining them sequentially can
    // deadlock when the child fills the stderr pipe while we are still
    // waiting for stdout EOF.
    let mut out_lines = child.stdout.take().map(OutputLines::new);
    let mut err_lines = child.stderr.take().map(OutputLines::new);
    let mut killed = false;
    let mut exited: Option<std::process::ExitStatus> = None;

    loop {
        tokio::select! {
            biased;
            _ = async {
                    match &cancel_token {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                }, if !killed && cancel_token.is_some() => {
                let _ = child.kill().await;
                killed = true;
            }
            line = async {
                match out_lines.as_mut() {
                    Some(lines) => lines.next_decoded_line().await,
                    None => Ok(None),
                }
            }, if out_lines.is_some() => {
                match line {
                    Ok(Some(line)) => {
                        if stdout.len() < MAX_STREAM_BYTES {
                            if !stdout.is_empty() {
                                stdout.push('\n');
                            }
                            // Reserve room for newline; truncate line itself.
                            let room = MAX_STREAM_BYTES.saturating_sub(stdout.len());
                            if line.len() + 1 > room {
                                stdout.push_str(&line[..room.saturating_sub(1).min(line.len())]);
                                stdout_truncated = true;
                            } else {
                                stdout.push_str(&line);
                            }
                        } else {
                            stdout_truncated = true;
                        }
                        if let Some(services) = &services {
                            if let Some(sink) = services.progress.as_ref() {
                                let _ = sink
                                    .progress(ctx.session_id, &ctx.tool_call_id, "bash", &line, "stdout")
                                    .await;
                            }
                        }
                    }
                    _ => out_lines = None,
                }
            }
            line = async {
                match err_lines.as_mut() {
                    Some(lines) => lines.next_decoded_line().await,
                    None => Ok(None),
                }
            }, if err_lines.is_some() => {
                match line {
                    Ok(Some(line)) => {
                        if stderr.len() < MAX_STREAM_BYTES {
                            if !stderr.is_empty() {
                                stderr.push('\n');
                            }
                            let room = MAX_STREAM_BYTES.saturating_sub(stderr.len());
                            if line.len() + 1 > room {
                                stderr.push_str(&line[..room.saturating_sub(1).min(line.len())]);
                                stderr_truncated = true;
                            } else {
                                stderr.push_str(&line);
                            }
                        } else {
                            stderr_truncated = true;
                        }
                        if let Some(services) = &services {
                            if let Some(sink) = services.progress.as_ref() {
                                let _ = sink
                                    .progress(ctx.session_id, &ctx.tool_call_id, "bash", &line, "stderr")
                                    .await;
                            }
                        }
                    }
                    _ => err_lines = None,
                }
            }
            // TERMINATION BRANCH — this must exist.
            //
            // The loop used to rely on `else => break`, which only fires when
            // every ENABLED branch has completed. The cancel branch stays
            // enabled whenever `cancel_token.is_some()` (always true in
            // production) and its future never completes, so once both pipes
            // reached EOF the select blocked forever: EVERY spawned command
            // hit its timeout, `echo hello` included. `child.wait()` returning
            // is the only reliable "the process is done" signal.
            status = child.wait() => {
                exited = Some(status.map_err(|e| ToolFailure {
                    message: e.to_string(),
                    retryable: true,
                })?);
                break;
            }
            else => break,
        }
    }

    // The process has exited: drain whatever the readers still hold. Bounded,
    // because an orphaned grandchild can keep the write end of a pipe open
    // (exactly what a timed-out `node`/`pnpm` leaves behind) and this drain
    // must never become a second hang. Output still buffered in the readers is
    // real output and must not be dropped.
    if let Some(mut lines) = out_lines.take() {
        let _ = timeout(Duration::from_millis(200), async {
            while let Ok(Some(line)) = lines.next_decoded_line().await {
                if stdout.len() < MAX_STREAM_BYTES {
                    if !stdout.is_empty() {
                        stdout.push('\n');
                    }
                    let room = MAX_STREAM_BYTES.saturating_sub(stdout.len());
                    if line.len() + 1 > room {
                        stdout.push_str(&line[..room.saturating_sub(1).min(line.len())]);
                        stdout_truncated = true;
                    } else {
                        stdout.push_str(&line);
                    }
                } else {
                    stdout_truncated = true;
                }
            }
        })
        .await;
    }
    if let Some(mut lines) = err_lines.take() {
        let _ = timeout(Duration::from_millis(200), async {
            while let Ok(Some(line)) = lines.next_decoded_line().await {
                if stderr.len() < MAX_STREAM_BYTES {
                    if !stderr.is_empty() {
                        stderr.push('\n');
                    }
                    let room = MAX_STREAM_BYTES.saturating_sub(stderr.len());
                    if line.len() + 1 > room {
                        stderr.push_str(&line[..room.saturating_sub(1).min(line.len())]);
                        stderr_truncated = true;
                    } else {
                        stderr.push_str(&line);
                    }
                } else {
                    stderr_truncated = true;
                }
            }
        })
        .await;
    }

    let status = match exited {
        Some(status) => status,
        // Loop ended without observing the exit (e.g. streams closed first).
        None => child.wait().await.map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?,
    };
    drop(_pid_guard);
    if stdout_truncated {
        stdout.push_str("\n…[stdout truncated at 1MiB during capture]");
    }
    if stderr_truncated {
        stderr.push_str("\n…[stderr truncated at 1MiB during capture]");
    }
    Ok(json!({
        "exit_code": status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "truncated": stdout_truncated || stderr_truncated,
    }))
}

/// Outcome of a PTY-backed run: either a finished value, a fatal tool error,
/// or `Unavailable` (ConPTY missing, spawn failed) telling the caller to fall
/// back to the pipe backend instead of failing the command.
enum PtyRunOutcome {
    Ok(Value),
    Unavailable(String),
    Err(ToolFailure),
}

/// Run `command` as the SOLE execution inside a fresh agent PTY.
///
/// Same argv the pipe backend would use (via `build_shell_argv`), spawned
/// non-interactively under the slave: no textual wrapping, no marker
/// protocol, exit status straight from `child.wait()`. Raw master bytes feed
/// the desktop display via broadcast; the LLM result accumulates decoded,
/// ANSI-stripped lines (the terminal merges stdout/stderr by design, so the
/// result carries the merged text in `stdout`).
async fn run_command_in_pty(
    tool: &BashTool,
    command: &str,
    timeout_ms: u64,
    ctx: &ToolContext,
) -> PtyRunOutcome {
    const MAX_STREAM_BYTES: usize = 1024 * 1024;
    let (program, argv) = build_shell_argv(tool.shell.as_deref(), command);
    let envs: Vec<(String, String)> = tool
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let registry = global_agent_pty_registry();
    let (session, mut child, mut rx) = match registry.spawn(
        &program,
        &argv,
        &ctx.working_directory,
        &envs,
    ) {
        Ok(tuple) => tuple,
        Err(message) => return PtyRunOutcome::Unavailable(message),
    };
    let pty_id = session.id.clone();

    let spawned_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
    if let Some(pid) = child.process_id() {
        spawned_pid.store(pid, std::sync::atomic::Ordering::SeqCst);
    }
    let services = current_services();
    if let Some(services) = &services {
        if let Some(sink) = services.progress.as_ref() {
            // Desktop attach handshake: a `stream == "pty"` progress chunk
            // carries the live session id. It is NEVER appended to any output
            // buffer (see `DetailStore::append_output`).
            let _ = sink
                .progress(
                    ctx.session_id,
                    &ctx.tool_call_id,
                    "bash",
                    &serde_json::json!({ "pty_id": pty_id }).to_string(),
                    "pty",
                )
                .await;
        }
    }
    let cancel_token = services.as_ref().and_then(|s| s.cancel.clone());
    let session_id = ctx.session_id;
    let tool_call_id = ctx.tool_call_id.clone();

    let output = match timeout(
        Duration::from_millis(timeout_ms),
        run_pty_loop(
            child,
            &mut rx,
            session_id,
            &tool_call_id,
            services.as_ref(),
            cancel_token.as_ref(),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            crate::process::kill_process_tree(Some(
                spawned_pid.load(std::sync::atomic::Ordering::SeqCst),
            ));
            registry.remove(&pty_id);
            return PtyRunOutcome::Err(ToolFailure {
                message: format!(
                    "command timed out after {timeout_ms}ms and was killed \
                     (its process tree too). Re-run with a larger `timeout_ms` \
                     (max 600000) if the command legitimately needs longer, \
                     or split it into smaller steps."
                ),
                retryable: true,
            });
        }
    };
    registry.remove(&pty_id);
    match output {
        Ok(value) => PtyRunOutcome::Ok(value),
        Err(failure) => PtyRunOutcome::Err(failure),
    }
}

/// Read loop for one agent PTY: decode incremental lines from raw master
/// bytes (ConPTY emits UTF-8), stream + accumulate them, exit on child end.
/// Mirrors `run_command_streaming`'s structure (cancel branch, termination
/// branch) so timeout/cancel semantics stay identical.
async fn run_pty_loop(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    rx: &mut tokio::sync::broadcast::Receiver<Vec<u8>>,
    session_id: code_plus_plus_types::SessionId,
    tool_call_id: &code_plus_plus_types::ToolCallId,
    services: Option<&super::services::ToolServices>,
    cancel_token: Option<&std::sync::Arc<super::services::CancelToken>>,
) -> std::result::Result<Value, ToolFailure> {
    const MAX_STREAM_BYTES: usize = 1024 * 1024;
    let mut merged = String::new();
    let mut truncated = false;
    // Carry-over for chunk splits mid-line / mid-rune.
    let mut pending: Vec<u8> = Vec::new();
    let mut killed = false;
    let mut exited: Option<portable_pty::ExitStatus> = None;

    // Decode one master chunk into result lines (ANSI-stripped) and report
    // each as `stdout` progress, exactly like the pipe backend reports lines.
    // Returns on broadcast lag (marks truncation, keeps going).
    async fn feed_chunk(
        data: Vec<u8>,
        pending: &mut Vec<u8>,
        merged: &mut String,
        truncated: &mut bool,
        session_id: code_plus_plus_types::SessionId,
        tool_call_id: &code_plus_plus_types::ToolCallId,
        services: Option<&super::services::ToolServices>,
    ) {
        pending.extend_from_slice(&data);
        let mut start = 0usize;
        let mut lines: Vec<String> = Vec::new();
        for (index, byte) in pending.iter().enumerate() {
            if *byte == b'\n' {
                let raw = pending[start..index].to_vec();
                start = index + 1;
                let text = String::from_utf8_lossy(&raw);
                let text = text.trim_end_matches('\r').to_string();
                lines.push(strip_ansi_escapes(&text));
            }
        }
        pending.drain(..start);
        for line in lines {
            if merged.len() < MAX_STREAM_BYTES {
                if !merged.is_empty() {
                    merged.push('\n');
                }
                let room = MAX_STREAM_BYTES.saturating_sub(merged.len());
                if line.len() + 1 > room {
                    merged.push_str(&line[..room.saturating_sub(1).min(line.len())]);
                    *truncated = true;
                } else {
                    merged.push_str(&line);
                }
            } else {
                *truncated = true;
            }
            if let Some(services) = &services {
                if let Some(sink) = services.progress.as_ref() {
                    let _ = sink
                        .progress(session_id, tool_call_id, "bash", &line, "stdout")
                        .await;
                }
            }
        }
    }

    loop {
        tokio::select! {
            biased;
            _ = async {
                    match &cancel_token {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                }, if !killed && cancel_token.is_some() => {
                let _ = child.kill();
                killed = true;
            }
            bytes = rx.recv() => {
                match bytes {
                    Ok(data) => {
                        feed_chunk(data, &mut pending, &mut merged, &mut truncated, session_id, tool_call_id, services).await;
                    }
                    // Lagged: reader outran us; mark truncation, keep going.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        truncated = true;
                    }
                    // Reader thread ended (EOF): the child is done or dying;
                    // keep polling it below instead of hanging here.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                exited = Some(status);
                                break;
                            }
                            _ => {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        }
                    }
                }
            }
            // `child.wait()` blocks a thread: poll instead so cancel/kill and
            // output draining stay responsive, same as the pipe backend.
            _ = tokio::time::sleep(Duration::from_millis(20)), if exited.is_none() => {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exited = Some(status);
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        break;
                    }
                }
            }
            else => break,
        }
    }

    // Drain whatever the reader still holds (bounded like the pipe drain).
    if !pending.is_empty() {
        let text = String::from_utf8_lossy(&pending);
        let text = text.trim_end_matches('\r').to_string();
        let line = strip_ansi_escapes(&text);
        if !line.is_empty() && merged.len() < MAX_STREAM_BYTES {
            if !merged.is_empty() {
                merged.push('\n');
            }
            merged.push_str(&line);
        }
        pending.clear();
    }
    let _ = rx;
    let status = match exited {
        Some(status) => status,
        None => {
            // Loop ended without observing the exit; reap on the blocking
            // pool so the async runtime is never stalled by a raw wait.
            // `child` is owned here (the loop released it), so moving it
            // into the closure is safe.
            match child.try_wait() {
                Ok(Some(status)) => status,
                _ => tokio::task::spawn_blocking(move || child.wait())
                    .await
                    .map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: true,
                    })?
                    .map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: true,
                    })?,
            }
        }
    };
    if truncated {
        merged.push_str("\n…[stdout truncated at 1MiB during capture]");
    }
    Ok(serde_json::json!({
        "exit_code": status.exit_code() as i64,
        "stdout": merged,
        "stderr": "",
        "truncated": truncated,
    }))
}

pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }
    fn description(&self) -> &str {
        "Apply a unified diff patch"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string" }
            },
            "required": ["patch"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "apply_patch",
            patch_bytes = input["patch"].as_str().map(|s| s.len()).unwrap_or(0),
            "apply_patch started"
        );
        ensure_not_cancelled()?;
        let patch = input["patch"].as_str().unwrap_or("").to_string();
        let working_directory = ctx.working_directory.clone();
        run_blocking_if_cancelled(move || {
            let file_patches = parse_unified_patch(&patch)?;
            let mut applied = Vec::new();
            let mut total_added = 0u32;
            let mut total_removed = 0u32;

            for (rel_path, hunks) in file_patches {
                ensure_not_cancelled()?;
                // Sandbox every file BEFORE any IO: absolute
                // `/etc/passwd` and `../../outside` must fail closed.
                // `validate_project_path` is symlink-aware (existing
                // prefixes canonicalized) so `link->/etc` escapes are blocked.
                let path = {
                    let base = PathBuf::from(&working_directory);
                    let p = PathBuf::from(&rel_path);
                    let joined = if p.is_absolute() { p } else { base.join(p) };
                    crate::project_context::validate_project_path(&working_directory, joined)?
                };
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                let updated = apply_hunks_to_content(&old, &hunks)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: false,
                    })?;
                    // Re-validate after parent creation (TOCTOU: parent may
                    // have been a symlink swapped between check and mkdir).
                    crate::project_context::validate_project_path(
                        &working_directory,
                        path.clone(),
                    )?;
                }
                ensure_not_cancelled()?;
                std::fs::write(&path, &updated).map_err(|e| ToolFailure {
                    message: e.to_string(),
                    retryable: false,
                })?;
                let stats = compute_text_diff_stats(&old, &updated);
                total_added += stats.added;
                total_removed += stats.removed;
                applied.push(path.display().to_string());
            }

            Ok(json!({
                "applied": !applied.is_empty(),
                "files": applied,
                "diff_stats": { "added": total_added, "removed": total_removed },
            }))
        })
        .await
        .inspect_err(|e| {
            tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %ctx.session_id,
                tool = "apply_patch",
                error = %e.message,
                "apply_patch failed"
            );
        })
    }
}

pub struct TodoWriteTool;

const TODOWRITE_DESCRIPTION: &str = include_str!("descriptions/todowrite.txt");

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todowrite"
    }
    fn description(&self) -> &str {
        TODOWRITE_DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string", "description": "pending, in_progress, completed, cancelled" },
                            "priority": { "type": "string", "description": "high, medium, low" }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "todowrite",
            todos = input["todos"].as_array().map(|a| a.len()).unwrap_or(0),
            "todowrite started"
        );
        use code_plus_plus_schema::todo::TodoItem;
        let items: Vec<TodoItem> = input["todos"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| TodoItem {
                        content: t["content"].as_str().unwrap_or("").to_string(),
                        status: t["status"].as_str().unwrap_or("pending").to_string(),
                        priority: t["priority"].as_str().unwrap_or("medium").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let store = resolve_todo_store(ctx)?;
        let services = current_services().ok_or_else(|| ToolFailure {
            message: "todo store not configured".into(),
            retryable: false,
        })?;
        let saved = store
            .update_todos(ctx.session_id, &items)
            .await
            .map_err(|e| ToolFailure {
                message: e,
                retryable: false,
            })?;
        emit_todo_updated(&services, ctx.session_id, &saved)
            .await
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
        Ok(json!({ "todos": saved }))
    }
}

pub struct TodoReadTool;

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todoread"
    }
    fn description(&self) -> &str {
        "Read the current session todo list"
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(
        &self,
        _input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "todoread",
            "todoread started"
        );
        let store = resolve_todo_store(ctx)?;
        let todos = store
            .get_todos(ctx.session_id)
            .await
            .map_err(|e| ToolFailure {
                message: e,
                retryable: false,
            })?;
        Ok(json!({ "todos": todos }))
    }
}

pub struct WebfetchTool;

#[async_trait]
impl Tool for WebfetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }
    fn description(&self) -> &str {
        "Fetch content from an HTTP(S) URL"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "HTTP(S) URL to fetch" }
            },
            "required": ["url"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::info!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            tool = "webfetch",
            url = %input["url"].as_str().unwrap_or(""),
            "webfetch started"
        );
        let url = input["url"].as_str().unwrap_or("");
        if url.is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                tool = "webfetch",
                "webfetch failed: empty url"
            );
            return Err(ToolFailure {
                message: "url must not be empty".into(),
                retryable: false,
            });
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolFailure {
                message: "url must start with http:// or https://".into(),
                retryable: false,
            });
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?;
        let response = client.get(url).send().await.map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
        let status = response.status().as_u16();
        // Bound body to 2MiB during download (P0-4 OOM guard).
        const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;
        let bytes = response.bytes().await.map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
        let truncated = bytes.len() > MAX_FETCH_BYTES;
        let body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_FETCH_BYTES)]).to_string();
        let body = if truncated {
            format!("{body}\n…[webfetch truncated at 2MiB]")
        } else {
            body
        };
        Ok(json!({
            "url": url,
            "status": status,
            "content": body,
            "truncated": truncated,
        }))
    }
}

/// A live language-server process shared by every LSP call for one project.
struct LspSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
    next_id: i64,
    /// Tail of `window/logMessage` output from the server, surfaced on the next
    /// response so project-resolution failures are visible.
    logs: Vec<String>,
    /// Set once `initialize` + `initialized` completed for this session.
    initialized: bool,
}

impl LspSession {
    /// Spawn a language server, resolving project-local `.cmd` shims and
    /// putting `node_modules/.bin` on PATH.
    async fn spawn(
        command: &[String],
        working_directory: &str,
        env: &HashMap<String, String>,
    ) -> std::result::Result<Self, ToolFailure> {
        let work_dir = std::path::PathBuf::from(working_directory);
        let (program, argv) = shell_env::resolve_program(&command[0], &command[1..], &work_dir);
        let mut child = Command::new(program);
        child.args(argv);
        child.env("PATH", shell_env::path_with_local_bins(&work_dir));
        child
            .current_dir(working_directory)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        shell_env::apply_to_command(&mut child, env);
        crate::process::hide_console_async(&mut child);

        let mut child = child.spawn().map_err(|e| ToolFailure {
            message: format!(
                "could not start lsp server `{}`: {e}. If it is a project devDependency it \
                 must be installed (`npm i -D <server>`); otherwise use the file, grep and \
                 glob tools instead of retrying.",
                command.join(" ")
            ),
            retryable: false,
        })?;
        let stdin = child.stdin.take().ok_or_else(|| ToolFailure {
            message: "failed to open lsp stdin".into(),
            retryable: false,
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolFailure {
            message: "failed to open lsp stdout".into(),
            retryable: false,
        })?;
        // Drain stderr continuously. Language servers log verbosely; leaving
        // the pipe unread lets it fill and block the server mid-request, which
        // presents as a dead session with empty results.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = reader.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            next_id: 0,
            logs: Vec::new(),
            initialized: false,
        })
    }
}

/// Is this an LSP *notification* (no response by protocol design) rather than a
/// request?
///
/// Sending a notification with an `id` makes the server treat it as a request;
/// `typescript-language-server` then answers `-32601 Unhandled method` because
/// it registered a notification handler. Waiting for a reply that never comes
/// also stalls the call until timeout.
fn is_notification(method: &str) -> bool {
    if method.starts_with("$/") {
        return true;
    }
    matches!(
        method,
        "initialized"
            | "exit"
            | "textDocument/didOpen"
            | "textDocument/didChange"
            | "textDocument/didClose"
            | "textDocument/didSave"
            | "textDocument/willSave"
            | "notebookDocument/didOpen"
            | "notebookDocument/didChange"
            | "notebookDocument/didSave"
            | "notebookDocument/didClose"
            | "workspace/didChangeConfiguration"
            | "workspace/didChangeWorkspaceFolders"
            | "workspace/didChangeWatchedFiles"
            | "workspace/didCreateFiles"
            | "workspace/didRenameFiles"
            | "workspace/didDeleteFiles"
            | "window/workDoneProgress/cancel"
    )
}

impl LspSession {
    async fn write_message(
        stdin: &mut tokio::process::ChildStdin,
        message: &Value,
    ) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let body = message.to_string();
        let payload = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        stdin.write_all(payload.as_bytes()).await?;
        stdin.flush().await
    }

    /// Read one framed JSON-RPC message.
    async fn read_message(
        stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    ) -> std::io::Result<Option<Value>> {
        use tokio::io::AsyncBufReadExt;
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let read = stdout.read_line(&mut line).await?;
            if read == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                if key.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse::<usize>().ok();
                }
            }
        }
        let Some(len) = content_length else {
            return Ok(None);
        };
        let mut body = vec![0u8; len];
        {
            use tokio::io::AsyncReadExt;
            stdout.read_exact(&mut body).await?;
        }
        Ok(serde_json::from_slice(&body).ok())
    }

    /// Answer a server→client request so the server never blocks on us.
    async fn answer_server_request(&mut self, message: &Value) -> std::io::Result<()> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(());
        };
        let method = message
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        // `workspace/configuration` expects one result per requested item, in
        // order. Returning `[]` for a non-empty `items` list makes servers
        // mis-read their own settings, which for TypeScript means the project
        // never gets configured.
        let result = if method == "workspace/configuration" {
            let count = message
                .get("params")
                .and_then(|p| p.get("items"))
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            json!(vec![Value::Null; count])
        } else {
            Value::Null
        };
        let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        Self::write_message(&mut self.stdin, &reply).await
    }

    /// Send a request and return the matching response, servicing any
    /// server→client traffic that arrives first.
    ///
    /// Server *notifications* (no `id`) are recorded rather than dropped:
    /// `window/logMessage` is how tsserver reports `No Project.` and similar
    /// project-resolution failures, and silently discarding it is what made
    /// these look like unexplained empty results.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        Self::write_message(&mut self.stdin, &message)
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let Some(reply) = Self::read_message(&mut self.stdout)
                .await
                .map_err(|e| e.to_string())?
            else {
                return Err("language server closed its output".into());
            };
            // Server→client request: answer it and keep waiting for ours.
            if reply.get("method").is_some() && reply.get("id").is_some() {
                self.answer_server_request(&reply)
                    .await
                    .map_err(|e| e.to_string())?;
                continue;
            }
            // Server notification (no id): keep it for diagnostics.
            if reply.get("method").is_some() {
                self.record_notification(&reply);
                continue;
            }
            if reply.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return Ok(self.attach_diagnostics(reply));
            }
            // Stale response from a timed-out request; ignore and keep reading.
        }
    }

    /// Remember the tail of server log/error messages.
    fn record_notification(&mut self, message: &Value) {
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if !matches!(
            method,
            "window/logMessage" | "textDocument/publishDiagnostics"
        ) {
            return;
        }
        let text = message
            .get("params")
            .and_then(|p| p.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        // Keep only the most recent few, and drop the routine startup banner.
        if text.contains("Using Typescript version") {
            return;
        }
        self.logs.push(text);
        if self.logs.len() > 8 {
            self.logs.remove(0);
        }
    }

    /// Hand the collected logs back to the caller alongside the result.
    fn attach_diagnostics(&mut self, mut response: Value) -> Value {
        if self.logs.is_empty() {
            return response;
        }
        let logs = std::mem::take(&mut self.logs);
        if let Some(obj) = response.as_object_mut() {
            obj.insert("serverLog".into(), json!(logs));
        }
        response
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        Self::write_message(&mut self.stdin, &message)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        // `kill_on_drop(true)` reaps the process; do not leave orphans.
        let _ = self.child.start_kill();
    }
}

/// Canonicalize a document reference so the server can map it to a tsconfig
/// project.
///
/// `typescript-language-server` resolves `textDocument.uri` against the real
/// filesystem. A hand-written URI with a lower-case drive, backslashes or a
/// missing percent-encoding fails that lookup and tsserver reports
/// `No Project.` — with every document feature then returning empty. The
/// model cannot be trusted to get this byte-exact, so we normalize it here.
///
/// Accepts `{"path": "src/a.ts"}` (preferred, resolved against the project)
/// or `{"uri": "file:///…"}` (normalized) and rewrites `params.textDocument`.
fn normalize_document_params(params: &mut Value, working_directory: &str) {
    let Some(td) = params.get_mut("textDocument") else {
        return;
    };
    if !td.is_object() {
        return;
    }
    let uri = td
        .get("uri")
        .and_then(|u| u.as_str())
        .map(|u| u.to_string())
        .or_else(|| {
            td.get("path")
                .and_then(|p| p.as_str())
                .map(|p| p.to_string())
        });
    let Some(uri) = uri else { return };
    let normalized = if let Some(rest) = uri.strip_prefix("file:") {
        // Strip any scheme/authority, then re-normalize the path portion.
        let path = rest.trim_start_matches("//").trim_start_matches('/');
        let decoded = percent_decode(path);
        canonical_file_uri(&decoded, working_directory)
    } else {
        // A bare path (relative or absolute).
        canonical_file_uri(&uri, working_directory)
    };
    td["uri"] = Value::String(normalized);
    if let Some(obj) = td.as_object_mut() {
        obj.remove("path");
    }
}

/// Absolute file URI for `path`, resolved against `working_directory` when
/// relative. Drive letters are upper-cased and separators normalised so the
/// URI matches how the server sees the filesystem.
fn canonical_file_uri(path: &str, working_directory: &str) -> String {
    let raw = path.trim();
    let mut absolute = if raw.starts_with('/') || has_drive_prefix(raw) {
        raw.to_string()
    } else {
        format!(
            "{}\\{}",
            working_directory.trim_end_matches(['\\', '/']),
            raw
        )
    };
    absolute = absolute.replace('/', "\\");
    // Upper-case the drive letter so the URI matches the casing the server
    // sees on disk; tsserver otherwise fails to match the file to its project.
    if has_drive_prefix(&absolute) {
        absolute.replace_range(0..1, &absolute[..1].to_ascii_uppercase());
    }
    // `C:` -> `C:/` so path_to_file_uri emits `file:///C:/…`.
    if has_drive_prefix(&absolute) && !absolute[2..].starts_with('\\') {
        absolute.insert(2, '\\');
    }
    path_to_file_uri(&absolute)
}

fn has_drive_prefix(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Fill in `textDocument.text` from disk when the caller omitted it.
///
/// A notification gives no response, so a content-less `didOpen` reports
/// success while leaving the server holding an empty buffer. Loading the file
/// here is what makes `hover`/`definition`/`documentSymbol` work.
fn hydrate_document_text(params: &mut Value) {
    let Some(td) = params.get_mut("textDocument") else {
        return;
    };
    let needs_text = td
        .get("text")
        .and_then(|t| t.as_str())
        .map(|t| t.is_empty())
        .unwrap_or(true);
    if !needs_text {
        return;
    }
    let Some(uri) = td.get("uri").and_then(|u| u.as_str()).map(String::from) else {
        return;
    };
    let Some(path) = file_uri_to_path(&uri) else {
        return;
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        td["text"] = Value::String(text);
    }
}

/// Coerce `"line": "29"` into `"line": 29`.
///
/// Models routinely emit the integers in an LSP `Position` as JSON strings.
/// The protocol requires numbers, and `typescript-language-server` answers a
/// string position with `null` — so `hover`/`definition` silently returned
/// nothing while `documentSymbol`, which takes no position, worked fine.
fn normalize_position_params(params: &mut Value) {
    if let Some(pos) = params.get_mut("position") {
        coerce_position(pos);
    }
    if let Some(ranges) = params.get_mut("ranges").and_then(|r| r.as_array_mut()) {
        for range in ranges {
            for edge in ["start", "end"] {
                if let Some(pos) = range.get_mut(edge) {
                    coerce_position(pos);
                }
            }
        }
    }
    if let Some(range) = params.get_mut("range") {
        for edge in ["start", "end"] {
            if let Some(pos) = range.get_mut(edge) {
                coerce_position(pos);
            }
        }
    }
}

fn coerce_position(pos: &mut Value) {
    let Some(obj) = pos.as_object_mut() else {
        return;
    };
    for key in ["line", "character"] {
        let coerced = match obj.get(key) {
            Some(Value::String(s)) => s.trim().parse::<i64>().ok().map(Value::from),
            Some(Value::Number(n)) => n.as_i64().map(Value::from).or(Some(Value::from(0))),
            _ => None,
        };
        if let Some(value) = coerced {
            obj.insert(key.to_string(), value);
        }
    }
}

/// Supply a `languageId` when the caller omitted it.
///
/// The LSP spec makes `languageId` required and servers reject `"undefined"`,
/// which shows up as a `serverLog` complaint and can make the server pick the
/// wrong grammar. Deriving it from the extension is what the caller means.
fn ensure_language_id(params: &mut Value, working_directory: &str) {
    let Some(td) = params.get_mut("textDocument") else {
        return;
    };
    let needs_id = td
        .get("languageId")
        .and_then(|v| v.as_str())
        .map(|s| s.is_empty() || s == "undefined")
        .unwrap_or(true);
    if !needs_id {
        return;
    }
    let ext = td
        .get("uri")
        .and_then(|u| u.as_str())
        .and_then(file_uri_to_path)
        .and_then(|p| p.extension().map(|e| e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let id = match ext.as_str() {
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "json" => "json",
        "py" | "pyi" => "python",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" | "zsh" => "shellscript",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        _ => {
            let _ = working_directory;
            "plaintext"
        }
    };
    td["languageId"] = Value::String(id.into());
}

/// Inverse of [`path_to_file_uri`] for the local `file:` URIs we generate.
fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = match rest.find('/') {
        Some(0) => rest.to_string(),
        Some(_) => return None, // non-empty authority: not a local file
        None => return None,
    };
    let mut decoded = String::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                decoded.push(b as char);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    // `file:///D:/…` keeps the leading slash; Windows wants it dropped.
    let trimmed = decoded
        .strip_prefix('/')
        .filter(|rest| {
            let b = rest.as_bytes();
            b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
        })
        .map(|rest| rest.to_string())
        .unwrap_or(decoded);
    Some(std::path::PathBuf::from(trimmed))
}

/// Public description text, kept out of the trait impl for testability.
pub fn lsp_tool_description() -> &'static str {
    "Talk to a language server over JSON-RPC stdio (e.g. \
     [\"typescript-language-server\",\"--stdio\"]). One server process is kept alive per project \
     and `initialize`/`initialized` are sent automatically on first use — never send them \
     yourself. REQUIRED ORDER: send `textDocument/didOpen` BEFORE any query about the file; \
     `typescript-language-server` only knows about files the client has opened, so this is what \
     makes `hover`, `definition`, `documentSymbol` and even `workspace/symbol` work. For any \
     `textDocument/*` method identify the file with `textDocument: {\"path\": \"src/a.ts\"}` \
     (relative to the project root) or `{\"uri\": \"file:///…\"}`; both are canonicalised, and \
     `didOpen` reads the file from disk for you when you omit `text`. Notifications (`$/…`, \
     *didOpen, *didChange) return immediately with {\"sent\":true}; requests return the server's \
     response, plus a `serverLog` array when the server reported anything."
}

pub struct LspTool {
    env: HashMap<String, String>,
    sessions: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<LspSession>>>>>,
}

impl LspTool {
    pub fn new(configured_shell: Option<&str>) -> Self {
        Self {
            env: shell_env::capture_shell_env(configured_shell),
            sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }
    fn description(&self) -> &str {
        lsp_tool_description()
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command and args to spawn the language server, e.g. [\"typescript-language-server\",\"--stdio\"]. Only needed on the first call of a session."
                },
                "method": { "type": "string", "description": "LSP method, e.g. textDocument/didOpen, textDocument/hover, workspace/symbol" },
                "params": {
                    "type": "object",
                    "description": "LSP params. For any textDocument/* method pass textDocument as {\"path\":\"src/a.ts\"} (relative to the project) or {\"uri\":\"file:///…\"}; both are canonicalised for you. Always didOpen a file before asking about it."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Give up waiting for the server after this many milliseconds. Default 20000, clamped to 1000..120000."
                }
            },
            "required": ["method"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "lsp",
            method = %input["method"].as_str().unwrap_or(""),
            "lsp started"
        );
        let method = input["method"].as_str().unwrap_or("").to_string();
        if method.is_empty() {
            return Err(ToolFailure {
                message: "method must not be empty".into(),
                retryable: false,
            });
        }
        let params = input.get("params").cloned().unwrap_or(json!({}));
        let timeout_ms = input["timeout_ms"]
            .as_u64()
            .unwrap_or(20_000)
            .clamp(1_000, 120_000);

        // One server per project, reused across calls. Spawning a fresh server
        // per call discarded the index and any `didOpen` state, so document
        // features failed with "Unexpected resource <uri>" and
        // `workspace/symbol` always came back empty.
        let session_key = format!("{}|{}", ctx.working_directory, method_is_shutdown(&method));
        if method == "shutdown" || method == "exit" {
            if let Ok(mut map) = self.sessions.lock() {
                if let Some(session) = map.remove(&ctx.working_directory) {
                    drop(session);
                }
            }
            return Ok(json!({ "stopped": true }));
        }

        let existing = self
            .sessions
            .lock()
            .ok()
            .and_then(|map| map.get(&ctx.working_directory).cloned());
        let session = match existing {
            Some(session) => session,
            None => {
                let command = input["command"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if command.is_empty() {
                    return Err(ToolFailure {
                        message: "command must be a non-empty array on the first call of a \
                                  session (e.g. [\"typescript-language-server\",\"--stdio\"])"
                            .into(),
                        retryable: false,
                    });
                }
                let created = Arc::new(tokio::sync::Mutex::new(
                    LspSession::spawn(&command, &ctx.working_directory, &self.env).await?,
                ));
                if let Ok(mut map) = self.sessions.lock() {
                    map.insert(ctx.working_directory.clone(), Arc::clone(&created));
                }
                let _ = session_key;
                created
            }
        };

        let mut guard = session.lock().await;
        // Canonicalize `textDocument` before it reaches the server. A
        // hand-written URI (lower-case drive, backslashes, unencoded spaces)
        // silently fails tsserver's project lookup and every document feature
        // comes back empty. `path` is accepted as a friendlier alternative.
        let mut params = params;
        if method.contains("textDocument/") {
            normalize_document_params(&mut params, &ctx.working_directory);
            normalize_position_params(&mut params);
        }
        // A `didOpen` with no `text` registers an *empty* document, and every
        // later query then quietly returns `[]`/`null` — the failure looks like
        // a broken server rather than a missing parameter. Models routinely
        // send only the URI, so read the file for them.
        if method == "textDocument/didOpen" {
            hydrate_document_text(&mut params);
            ensure_language_id(&mut params, &ctx.working_directory);
        }
        // Handshake on first use so the workspace is indexed and the client
        // capabilities are declared; without it `workspace/symbol` returns [].
        if !guard.initialized {
            let root_uri = path_to_file_uri(&ctx.working_directory);
            let init = json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "rootPath": ctx.working_directory,
                "workspaceFolders": [{
                    "uri": path_to_file_uri(&ctx.working_directory),
                    "name": "workspace"
                }],
                "capabilities": {
                    "textDocument": {
                        "synchronization": { "dynamicRegistration": false },
                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                        "definition": { "linkSupport": true },
                        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true }
                    },
                    "workspace": { "symbol": {}, "workspaceFolders": true },
                    "window": { "workDoneProgress": true }
                },
                "clientInfo": { "name": code_plus_plus_types::PRODUCT_NAME, "version": env!("CARGO_PKG_VERSION") }
            });
            let initialized = timeout(
                Duration::from_millis(timeout_ms),
                guard.request("initialize", init),
            )
            .await
            .map_err(|_| ToolFailure {
                message: format!(
                    "lsp server did not answer `initialize` within {timeout_ms}ms. \
                         The command is probably not a language server."
                ),
                retryable: false,
            })?
            .map_err(|e| ToolFailure {
                message: format!("lsp initialize failed: {e}"),
                retryable: false,
            })?;
            if let Some(err) = initialized.get("error") {
                return Err(ToolFailure {
                    message: format!("lsp initialize rejected: {err}"),
                    retryable: false,
                });
            }
            // `initialized` is a notification: no id, no reply.
            let _ = timeout(
                Duration::from_millis(2_000),
                guard.notify("initialized", json!({})),
            )
            .await;
            guard.initialized = true;
        }

        if is_notification(&method) {
            guard
                .notify(&method, params)
                .await
                .map_err(|e| ToolFailure {
                    message: format!("lsp notification `{method}` failed: {e}"),
                    retryable: false,
                })?;
            tracing::debug!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %ctx.session_id,
                tool = "lsp",
                method = %method,
                "lsp notification sent"
            );
            return Ok(json!({ "sent": true, "notification": method }));
        }

        let response = timeout(
            Duration::from_millis(timeout_ms),
            guard.request(&method, params),
        )
        .await
        .map_err(|_| ToolFailure {
            message: format!(
                "lsp did not respond to `{method}` within {timeout_ms}ms. If no language \
                 server is installed, do not retry — use the file/grep/glob tools."
            ),
            retryable: false,
        })?
        .map_err(|e| ToolFailure {
            message: format!("lsp request `{method}` failed: {e}"),
            retryable: false,
        })?;
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "lsp",
            method = %method,
            "lsp completed"
        );
        Ok(response)
    }
}

/// `true` when the caller asked to stop the session (used only for logging).
fn method_is_shutdown(method: &str) -> bool {
    matches!(method, "shutdown" | "exit")
}

/// `D:\a\b` → `file:///D:/a/b` (LSP requires percent-encoded, forward-slash URIs).
fn path_to_file_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/');
    let mut out = String::from("file:///");
    for ch in trimmed.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => out.push(ch),
            ' ' => out.push_str("%20"),
            other => {
                let mut buf = [0u8; 4];
                for byte in other.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

fn resolve_todo_store(
    ctx: &ToolContext,
) -> std::result::Result<std::sync::Arc<dyn TodoStoreOps>, ToolFailure> {
    if let Some(store) = ctx.extras.todo_store.clone() {
        return Ok(store);
    }
    let services = current_services().ok_or_else(|| ToolFailure {
        message: "todo store not configured".into(),
        retryable: false,
    })?;
    let store = services.todo_store.clone().ok_or_else(|| ToolFailure {
        message: "todo store not configured".into(),
        retryable: false,
    })?;
    Ok(Arc::new(crate::todo::StorageTodoStoreOps(store)))
}

pub(crate) fn resolve_path(
    ctx: &ToolContext,
    path: &str,
) -> std::result::Result<PathBuf, ToolFailure> {
    let p = PathBuf::from(path);
    let resolved = if p.is_absolute() {
        p
    } else {
        PathBuf::from(&ctx.working_directory).join(p)
    };
    crate::project_context::validate_project_path(&ctx.working_directory, resolved)
}

fn shell_base_name(shell: &str) -> String {
    let lower = shell.to_lowercase();
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    // Windows executables carry a `.exe` suffix. `C:\Program Files\Git\bin\
    // bash.exe` must still be recognised as `bash`, otherwise the shell falls
    // through to the cmd.exe `/C` branch and every command fails instantly
    // with an invalid-option error —a fully broken bash tool on Windows.
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".cmd"))
        .or_else(|| name.strip_suffix(".bat"))
        .unwrap_or(name)
        .to_string()
}

fn is_powershell(shell: &str) -> bool {
    let name = shell_base_name(shell);
    name.contains("powershell") || name == "pwsh" || name == "pwsh-preview"
}

/// Shells that accept `-lc` (login + command) argument style.
fn is_posix_style_shell(shell: &str) -> bool {
    matches!(
        shell_base_name(shell).as_str(),
        "bash" | "sh" | "zsh" | "dash" | "ksh" | "ash"
    )
}

#[derive(Debug, Clone)]
struct PatchHunk {
    old_start: usize,
    lines: Vec<(char, String)>,
}

fn parse_unified_patch(
    patch: &str,
) -> std::result::Result<Vec<(String, Vec<PatchHunk>)>, ToolFailure> {
    let mut files: Vec<(String, Vec<PatchHunk>)> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_hunks: Vec<PatchHunk> = Vec::new();
    let mut current_hunk: Option<PatchHunk> = None;

    let flush_file =
        |file: Option<String>, hunks: Vec<PatchHunk>, files: &mut Vec<(String, Vec<PatchHunk>)>| {
            if let Some(file) = file {
                if !hunks.is_empty() {
                    files.push((file, hunks));
                }
            }
        };

    for line in patch.lines() {
        if line.starts_with("+++ ") {
            // The in-progress hunk still belongs to the previous file; flush it
            // together with the file, otherwise it would be misattributed to the
            // next file (or silently dropped).
            if let Some(hunk) = current_hunk.take() {
                current_hunks.push(hunk);
            }
            flush_file(
                current_file.take(),
                std::mem::take(&mut current_hunks),
                &mut files,
            );
            let rel = line
                .trim_start_matches("+++ ")
                .trim_start_matches("b/")
                .trim_start_matches("a/")
                .to_string();
            current_file = Some(rel);
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff ") || line.starts_with("index ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                current_hunks.push(hunk);
            }
            let header = rest.split(" @@").next().unwrap_or(rest);
            let old_start = header
                .split_whitespace()
                .next()
                .and_then(|part| part.strip_prefix('-'))
                .and_then(|range| range.split(',').next())
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(1);
            current_hunk = Some(PatchHunk {
                old_start,
                lines: Vec::new(),
            });
            continue;
        }
        if let Some(hunk) = current_hunk.as_mut() {
            if let Some(first) = line.chars().next() {
                if matches!(first, ' ' | '-' | '+') {
                    hunk.lines.push((first, line[1..].to_string()));
                }
            }
        }
    }

    if let Some(hunk) = current_hunk.take() {
        current_hunks.push(hunk);
    }
    flush_file(current_file.take(), current_hunks, &mut files);

    if files.is_empty() {
        return Err(ToolFailure {
            message: "patch contained no applicable hunks".into(),
            retryable: true,
        });
    }
    Ok(files)
}

fn apply_hunks_to_content(
    content: &str,
    hunks: &[PatchHunk],
) -> std::result::Result<String, ToolFailure> {
    let mut lines: Vec<String> = if content.is_empty() {
        Vec::new()
    } else {
        content.lines().map(String::from).collect()
    };
    let had_trailing_newline = content.ends_with('\n');

    for hunk in hunks {
        let pos = find_hunk_position(&lines, hunk)?;
        let mut offset = pos;
        for (kind, text) in &hunk.lines {
            match kind {
                ' ' => {
                    if lines.get(offset).map(String::as_str) != Some(text.as_str()) {
                        return Err(ToolFailure {
                            message: format!("patch context mismatch at line {}", offset + 1),
                            retryable: true,
                        });
                    }
                    offset += 1;
                }
                '-' => {
                    if lines.get(offset).map(String::as_str) != Some(text.as_str()) {
                        return Err(ToolFailure {
                            message: format!("patch remove mismatch at line {}", offset + 1),
                            retryable: true,
                        });
                    }
                    lines.remove(offset);
                }
                '+' => {
                    lines.insert(offset, text.clone());
                    offset += 1;
                }
                _ => {}
            }
        }
    }

    let mut out = lines.join("\n");
    if had_trailing_newline || out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn find_hunk_position(
    lines: &[String],
    hunk: &PatchHunk,
) -> std::result::Result<usize, ToolFailure> {
    let context: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|(kind, text)| {
            if *kind == ' ' {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    if context.is_empty() {
        return Ok(hunk.old_start.saturating_sub(1));
    }

    let start = hunk.old_start.saturating_sub(1);
    let max_start = lines.len();
    for delta in 0..=max_start {
        for sign in [0isize, -1, 1] {
            let candidate = if sign == 0 {
                start + delta
            } else if sign < 0 {
                start.saturating_sub(delta)
            } else {
                start + delta
            };
            if candidate > lines.len() {
                continue;
            }
            if hunk_matches_at(lines, hunk, candidate) {
                return Ok(candidate);
            }
        }
    }

    Err(ToolFailure {
        message: format!("unable to locate patch hunk near line {}", hunk.old_start),
        retryable: true,
    })
}

fn hunk_matches_at(lines: &[String], hunk: &PatchHunk, pos: usize) -> bool {
    let mut offset = pos;
    for (kind, text) in &hunk.lines {
        match kind {
            ' ' | '-' => {
                if lines.get(offset).map(String::as_str) != Some(text.as_str()) {
                    return false;
                }
                offset += 1;
            }
            '+' => {}
            _ => {}
        }
    }
    true
}

/// On Windows, dev-server commands block the bash tool until timeout. Run them detached.
fn prepare_background_bash_command(command: &str) -> String {
    if !cfg!(windows) {
        return command.to_string();
    }
    let lower = command.to_lowercase();
    let is_dev_server = (lower.contains(" next dev")
        || lower.contains(" npm run dev")
        || lower.contains("pnpm dev")
        || lower.contains("yarn dev")
        || lower.contains(" vite"))
        && !lower.contains("next build")
        && !lower.contains("vite build");
    let already_background =
        lower.contains("start /b") || lower.contains("start /min") || lower.contains("nohup ");
    if is_dev_server && !already_background {
        format!("start /B {command}")
    } else {
        command.to_string()
    }
}

#[cfg(test)]
mod lsp_protocol_tests {
    use serde_json::json;

    use super::{
        file_uri_to_path, hydrate_document_text, is_notification, lsp_tool_description,
        normalize_document_params, normalize_position_params, path_to_file_uri,
    };

    /// Regression: notifications were sent WITH an `id`, so
    /// `typescript-language-server` treated `didOpen` as a request and replied
    /// `-32601 Unhandled method` — which is why no document feature worked.
    #[test]
    fn notifications_are_classified_without_an_id() {
        for method in [
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didClose",
            "initialized",
            "workspace/didChangeConfiguration",
            "$/cancelRequest",
        ] {
            assert!(is_notification(method), "{method} must be a notification");
        }
    }

    #[test]
    fn requests_are_not_treated_as_notifications() {
        for method in [
            "initialize",
            "textDocument/hover",
            "textDocument/definition",
            "textDocument/documentSymbol",
            "workspace/symbol",
            "workspace/executeCommand",
        ] {
            assert!(!is_notification(method), "{method} must be a request");
        }
    }

    /// `rootUri` must be a valid file URI or the server indexes nothing and
    /// `workspace/symbol` comes back empty.
    #[test]
    fn path_to_file_uri_handles_windows_and_spaces() {
        assert_eq!(
            path_to_file_uri(r"D:\Workspace\Echoo"),
            "file:///D:/Workspace/Echoo"
        );
        assert_eq!(path_to_file_uri(r"C:\a b\c.ts"), "file:///C:/a%20b/c.ts");
        assert!(path_to_file_uri("/home/u/p").starts_with("file:///home/u/p"));
    }

    /// The ordering requirement must be in the description the model reads,
    /// otherwise it queries a document it never opened and reports LSP broken.
    #[test]
    fn description_states_the_didopen_precondition() {
        let d = lsp_tool_description();
        assert!(d.contains("didOpen"), "{d}");
        assert!(d.contains("BEFORE"), "{d}");
    }

    /// A lower-case drive letter is enough for tsserver to fail the project
    /// lookup and return `No Project.` with every document feature empty.
    #[test]
    fn document_uri_is_canonicalised_regardless_of_how_it_was_written() {
        let dir = r"D:\Workspace\Echoo";

        let mut p =
            json!({"textDocument":{"uri":"file:///d:/workspace/echoo/apps/mobile/src/a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(
            p["textDocument"]["uri"],
            json!("file:///D:/workspace/echoo/apps/mobile/src/a.ts")
        );

        // Backslashes instead of forward slashes.
        let mut p = json!({"textDocument":{"uri":r"file:///D:\Workspace\Echoo\a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(
            p["textDocument"]["uri"],
            json!("file:///D:/Workspace/Echoo/a.ts")
        );

        // Percent-encoded spaces survive a decode/re-encode round trip.
        let mut p = json!({"textDocument":{"uri":"file:///D:/Work%20space/a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(
            p["textDocument"]["uri"],
            json!("file:///D:/Work%20space/a.ts")
        );
    }

    /// `path` is the safer input: the model cannot get a URI wrong.
    #[test]
    fn document_path_resolves_against_the_project() {
        let dir = r"D:\Workspace\Echoo";

        let mut p = json!({"textDocument":{"path":"apps/mobile/src/a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(
            p["textDocument"]["uri"],
            json!("file:///D:/Workspace/Echoo/apps/mobile/src/a.ts")
        );
        assert!(p["textDocument"].get("path").is_none());

        // Absolute paths and drive-relative roots keep working.
        let mut p = json!({"textDocument":{"path":r"D:\other\a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(p["textDocument"]["uri"], json!("file:///D:/other/a.ts"));

        let mut p = json!({"textDocument":{"path":"/home/u/a.ts"}});
        normalize_document_params(&mut p, dir);
        assert_eq!(p["textDocument"]["uri"], json!("file:///home/u/a.ts"));
    }

    /// Params without a `textDocument` (workspace/*) must pass through
    /// untouched rather than panic.
    #[test]
    fn normalization_leaves_other_params_alone() {
        let dir = r"D:\Workspace\Echoo";

        let mut p = json!({"query":"Foo"});
        normalize_document_params(&mut p, dir);
        assert_eq!(p, json!({"query":"Foo"}));

        let mut p = json!({});
        normalize_document_params(&mut p, dir);
        assert_eq!(p, json!({}));
    }

    /// The regression that made LSP look broken: a `didOpen` with no `text`
    /// registers an empty document and every later query returns nothing.
    #[test]
    fn didopen_without_text_is_loaded_from_disk() {
        let dir = std::env::temp_dir().join(format!("lsp-hydrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        std::fs::write(&file, "export const alpha = 1;\n").unwrap();
        let uri = path_to_file_uri(file.to_str().unwrap());

        let mut p = json!({"textDocument":{"uri":uri,"languageId":"typescript","version":1}});
        hydrate_document_text(&mut p);
        assert_eq!(
            p["textDocument"]["text"],
            json!("export const alpha = 1;\n")
        );

        // Caller-supplied content wins: never clobber unsaved buffers.
        let mut p = json!({"textDocument":{"uri":uri,"text":"draft"}});
        hydrate_document_text(&mut p);
        assert_eq!(p["textDocument"]["text"], json!("draft"));

        // An unreadable path must not invent content or panic.
        let mut p = json!({"textDocument":{"uri":"file:///definitely/not/here.ts"}});
        hydrate_document_text(&mut p);
        assert!(p["textDocument"].get("text").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bash_execution_tests {
    use super::*;
    use code_plus_plus_schema::tool::ToolContextExtras;
    use code_plus_plus_types::{MessageId, SessionId, ToolCallId};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            session_id: SessionId::new(),
            agent: "build".into(),
            assistant_message_id: MessageId::new(),
            tool_call_id: ToolCallId::new(),
            working_directory: dir.to_string_lossy().to_string(),
            extras: ToolContextExtras { todo_store: None },
        }
    }

    /// End-to-end: the bash tool must actually run a trivial command and
    /// return its stdout. `echo hello` is the smallest possible proof that
    /// the shell works; everything the LLM concluded about "shell broken" was
    /// downstream of this.
    #[tokio::test]
    async fn echo_hello_runs_and_returns_stdout() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let out = tool
            .execute(
                json!({ "command": "echo hello", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("bash must succeed");
        let text = out.to_string();
        assert!(
            text.contains("hello"),
            "stdout missing 'hello'; tool returned: {text}"
        );
    }

    /// The default Windows path wraps the command in `chcp 65001 >nul && …`.
    /// That wrapper must not swallow or corrupt the command's own output.
    #[cfg(windows)]
    #[tokio::test]
    async fn echo_hello_survives_the_utf8_wrapper() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let out = tool
            .execute(
                json!({ "command": "echo hello", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("bash must succeed");
        let text = out.to_string();
        assert!(text.contains("hello"), "{text}");
        // The wrapper's own `chcp` chatter must not appear in the result.
        assert!(!text.contains("Active code page"), "{text}");
    }

    /// A configured Git Bash must receive `-lc`, not cmd's `/C`.
    #[cfg(windows)]
    #[tokio::test]
    async fn configured_git_bash_executes_commands() {
        let bash = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
        let Some(bash) = bash else {
            // Git Bash not installed on this machine; detection is covered by
            // the pure unit tests.
            return;
        };
        let dir = std::env::temp_dir();
        let tool = BashTool::new(Some(bash.to_string()), HashMap::new());
        let out = tool
            .execute(
                json!({ "command": "echo hello", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("configured bash must succeed");
        let text = out.to_string();
        assert!(text.contains("hello"), "Git Bash returned: {text}");
        assert!(!text.contains("invalid option"), "argv style wrong: {text}");
    }

    /// THE decisive regression: the read loop's `select!` had **no
    /// `child.wait()` branch**, and its `else => break` only fires when every
    /// enabled branch has completed. The cancel branch is enabled whenever
    /// `cancel_token.is_some()` and its future never completes, so with
    /// services installed (production) the select blocked forever once both
    /// pipes hit EOF —EVERY spawned command hit the timeout, even
    /// `echo hello`.
    ///
    /// The earlier tests all passed because `current_services()` is `None`
    /// there, which disables the cancel branch and lets `else` break. This one
    /// installs a real cancel token, i.e. the production condition.
    #[tokio::test]
    async fn bash_completes_with_a_cancel_token_installed() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let cancel = Arc::new(crate::tools::services::CancelToken::new());
        let services = crate::tools::services::ToolServices {
            progress: None,
            todo_store: None,
            events: None,
            question_tx: None,
            task_spawner: None,
            cancel: Some(cancel),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::tools::services::with_services_async(services, async {
                tool.execute(
                    json!({ "command": "echo hello", "timeout_ms": 15_000 }),
                    &ctx(&dir),
                )
                .await
            }),
        )
        .await;
        let out = result
            .expect("bash deadlocked: select! never observed the process exiting")
            .expect("bash must succeed");
        assert!(out.to_string().contains("hello"), "{}", out);
    }

    /// PTY backend: trivial echo through a REAL terminal. The merged stream
    /// lands in `stdout` with exit 0 — the contract the desktop live tab
    /// relies on. (Opt-in per instance via `.with_pty(true)`; the registry
    /// enables it from `CODE_PLUS_PLUS_BASH_PTY=1`. Never via env here: env is
    /// process-global and would race parallel tests.)
    #[tokio::test]
    async fn pty_echo_returns_merged_stdout_and_zero() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let out = tool
            .execute(
                json!({ "command": "echo hello-pty", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("pty bash must succeed");
        let stdout = out["stdout"].as_str().unwrap_or_default().to_string();
        assert!(stdout.contains("hello-pty"), "{out}");
        assert_eq!(out["exit_code"].as_i64(), Some(0), "{out}");
        assert_eq!(out["stderr"].as_str().unwrap_or("?"), "", "{out}");
    }

    /// PTY backend propagates non-zero exit codes without any marker
    /// protocol: the status comes straight from `child.wait()`.
    #[tokio::test]
    async fn pty_exit_code_propagates() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let out = tool
            .execute(json!({ "command": "exit 3", "timeout_ms": 30_000 }), &ctx(&dir))
            .await
            .expect("pty run itself must resolve");
        assert_eq!(out["exit_code"].as_i64(), Some(3), "{out}");
    }

    /// PTY merges stderr into the single terminal stream BY DESIGN (a
    /// console has one screen): `1>&2` output shows up inside `stdout`.
    #[tokio::test]
    async fn pty_stderr_merges_into_stdout() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let out = tool
            .execute(
                json!({ "command": "echo oops-pty 1>&2", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("pty bash must succeed");
        let stdout = out["stdout"].as_str().unwrap_or_default().to_string();
        assert!(stdout.contains("oops-pty"), "{out}");
    }

    /// PTY result text is ANSI-stripped even when the program colorizes for
    /// TTYs (the live display keeps raw bytes; only the result is cleaned).
    #[tokio::test]
    async fn pty_result_strips_ansi_colors() {
        let dir = std::env::temp_dir();
        let command = if cfg!(windows) {
            "echo \u{1b}[31mred-pty\u{1b}[0m"
        } else {
            "printf '\\033[31mred-pty\\033[0m\\n'"
        };
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let out = tool
            .execute(json!({ "command": command, "timeout_ms": 30_000 }), &ctx(&dir))
            .await
            .expect("pty bash must succeed");
        let stdout = out["stdout"].as_str().unwrap_or_default().to_string();
        assert!(stdout.contains("red-pty"), "{out}");
        assert!(!stdout.contains('\x1b'), "ANSI leaked into result: {out:?}");
    }

    /// PTY timeout kills the whole tree and reports the standard message
    /// (same text as the pipe backend so callers need no new handling).
    #[tokio::test]
    async fn pty_timeout_kills_and_reports() {
        let dir = std::env::temp_dir();
        let command = if cfg!(windows) {
            "ping -n 30 127.0.0.1 >nul"
        } else {
            "sleep 30"
        };
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let err = tool
            .execute(json!({ "command": command, "timeout_ms": 1_500 }), &ctx(&dir))
            .await
            .expect_err("overlong pty command must time out");
        assert!(err.message.contains("timed out"), "{}", err.message);
        assert!(err.retryable, "timeout must stay retryable");
    }

    /// PTY backend completes with a cancel token installed (production
    /// condition): the polling `try_wait` branch must observe the exit just
    /// like the pipe backend's `child.wait()` branch does.
    #[tokio::test]
    async fn pty_completes_with_a_cancel_token_installed() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new()).with_pty(true);
        let cancel = Arc::new(crate::tools::services::CancelToken::new());
        let services = crate::tools::services::ToolServices {
            progress: None,
            todo_store: None,
            events: None,
            question_tx: None,
            task_spawner: None,
            cancel: Some(cancel),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::tools::services::with_services_async(services, async {
                tool.execute(
                    json!({ "command": "echo hello-cancel", "timeout_ms": 20_000 }),
                    &ctx(&dir),
                )
                .await
            }),
        )
        .await;
        let out = result
            .expect("pty bash deadlocked with cancel token installed")
            .expect("pty bash must succeed");
        assert!(out.to_string().contains("hello-cancel"), "{}", out);
    }

    /// `BufRead::lines()` fails on the first non-UTF-8 byte, and the reader
    /// then dropped the ENTIRE remaining stream. One Chinese character in
    /// `dir` output therefore erased every later byte — including plain ASCII.
    #[tokio::test]
    async fn non_utf8_output_does_not_discard_the_stream() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let out = tool
            .execute(
                json!({ "command": "echo \u{4e16}\u{754c} && echo MARKER_AFTER", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("bash must succeed");
        let stdout = out["stdout"].as_str().unwrap_or_default().to_string();
        assert!(
            stdout.contains("MARKER_AFTER"),
            "ASCII following non-UTF-8 output was dropped; stdout={stdout:?}"
        );
        assert!(
            stdout.contains('\u{4e16}'),
            "non-ASCII text was mangled; stdout={stdout:?}"
        );
    }

    /// A decoding problem must never be reported as a command failure: the
    /// exit code has to be the child's real status.
    #[tokio::test]
    async fn exit_code_is_never_fabricated_for_undecodable_output() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let out = tool
            .execute(
                json!({ "command": "echo \u{4e16}\u{754c} && exit 0", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("bash must succeed");
        assert_eq!(
            out["exit_code"].as_i64(),
            Some(0),
            "decodable command reported a bogus exit code: {out}"
        );
    }

    /// `Encoding::for_label("cp936")` returns `None`, so the code page must be
    /// mapped by number or GBK output decodes as Windows-1252 mojibake.
    #[cfg(windows)]
    #[test]
    fn code_page_936_decodes_gbk() {
        // "当前代码页" encoded as GBK.
        let gbk = [0xB5u8, 0xB1, 0xC7, 0xB0, 0xB4, 0xFA, 0xC2, 0xEB, 0xD2, 0xB3];
        let (text, _, had_errors) = encoding_for_code_page(936).decode(&gbk);
        assert!(!had_errors, "GBK bytes did not decode cleanly");
        assert_eq!(text, "\u{5f53}\u{524d}\u{4ee3}\u{7801}\u{9875}");
    }

    #[cfg(windows)]
    #[test]
    fn code_page_65001_is_utf8() {
        assert_eq!(encoding_for_code_page(65001).name(), "UTF-8");
    }

    /// A line may straddle two reads, and its bytes may be split mid
    /// character — common for large `dir` output.
    #[tokio::test]
    async fn reader_handles_split_multibyte_sequences() {
        let mut reader = OutputLines::new(std::io::Cursor::new(
            "first\nsecond line\nlast".as_bytes().to_vec(),
        ));
        assert_eq!(
            reader.next_decoded_line().await.unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            reader.next_decoded_line().await.unwrap().as_deref(),
            Some("second line")
        );
        assert_eq!(
            reader.next_decoded_line().await.unwrap().as_deref(),
            Some("last")
        );
        assert_eq!(reader.next_decoded_line().await.unwrap(), None);
    }

    /// Regression: a command whose output is only decodable through the OEM code
    /// page reported a bogus exit code and lost its output.
    #[cfg(windows)]
    #[tokio::test]
    async fn legacy_code_page_output_keeps_its_real_exit_code() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(None, HashMap::new());
        let out = tool
            .execute(
                // `chcp 936` switches cmd to GBK for the rest of the line, so
                // the Chinese banner and the ASCII after it exercise both paths.
                json!({ "command": "chcp 936 && echo \u{4e16}\u{754c} && exit 0", "timeout_ms": 30_000 }),
                &ctx(&dir),
            )
            .await
            .expect("bash must succeed");
        assert_eq!(
            out["exit_code"].as_i64(),
            Some(0),
            "GBK command reported a bogus exit code: {out}"
        );
        let stdout = out["stdout"].as_str().unwrap_or_default().to_string();
        assert!(
            stdout.contains('\u{4e16}'),
            "GBK output was dropped: {stdout:?}"
        );
    }

    #[test]
    fn utf8_is_preferred_and_gbk_is_the_fallback() {
        assert_eq!(
            decode_console_bytes("\u{4e16}\u{754c}".as_bytes()),
            "\u{4e16}\u{754c}"
        );
        // 世界 encoded as GBK.
        assert_eq!(
            decode_console_bytes(&[0xCAu8, 0xC0, 0xBD, 0xE7]),
            "\u{4e16}\u{754c}"
        );
    }

    /// The reader strips the line terminator; the caller re-joins with `\n`,
    /// so keeping it produced `\r\n\n` in every captured stream.
    #[tokio::test]
    async fn lines_carry_no_trailing_terminator() {
        let mut reader = OutputLines::new(std::io::Cursor::new(b"a\r\nb\r\n".to_vec()));
        assert_eq!(
            reader.next_decoded_line().await.unwrap().as_deref(),
            Some("a")
        );
        assert_eq!(
            reader.next_decoded_line().await.unwrap().as_deref(),
            Some("b")
        );
        assert_eq!(reader.next_decoded_line().await.unwrap(), None);
        assert_eq!(trim_cr(b"a\r"), b"a");
        assert_eq!(trim_cr(b"a"), b"a");
    }

    /// Models emit LSP `Position` integers as JSON strings. The protocol
    /// requires numbers and `typescript-language-server` answers a string
    /// position with `null`, so `hover`/`definition` returned nothing while
    /// `documentSymbol` (which takes no position) worked.
    #[test]
    fn string_positions_are_coerced_to_numbers() {
        let mut p = json!({"position":{"character":"25","line":"29"}});
        normalize_position_params(&mut p);
        assert_eq!(p["position"]["line"], json!(29));
        assert_eq!(p["position"]["character"], json!(25));

        // Already-numeric positions are left alone.
        let mut p = json!({"position":{"character":7,"line":3}});
        normalize_position_params(&mut p);
        assert_eq!(p["position"]["line"], json!(3));
        assert_eq!(p["position"]["character"], json!(7));

        // Nested range shapes used by definition/references/formatting.
        let mut p = json!({"ranges":[{"start":{"line":"1","character":"2"},
                                     "end":{"line":"3","character":"4"}}]});
        normalize_position_params(&mut p);
        assert_eq!(p["ranges"][0]["start"]["line"], json!(1));
        assert_eq!(p["ranges"][0]["end"]["character"], json!(4));

        let mut p = json!({"range":{"start":{"line":"5","character":"0"},
                                    "end":{"line":"6","character":"0"}}});
        normalize_position_params(&mut p);
        assert_eq!(p["range"]["start"]["line"], json!(5));
        assert_eq!(p["range"]["end"]["line"], json!(6));

        // Params without a position must pass through untouched.
        let mut p = json!({"textDocument":{"uri":"file:///a.ts"}});
        normalize_position_params(&mut p);
        assert_eq!(p, json!({"textDocument":{"uri":"file:///a.ts"}}));

        // Non-numeric junk is left alone rather than silently zeroed.
        let mut p = json!({"position":{"line":"abc"}});
        normalize_position_params(&mut p);
        assert_eq!(p["position"]["line"], json!("abc"));
    }

    /// `languageId` is required by the spec; servers log
    /// `Invalid languageId "undefined"` and may pick the wrong grammar.
    #[test]
    fn didopen_infers_a_language_id() {
        let dir = r"D:\Workspace\Echoo";

        let mut p = json!({"textDocument":{"uri":"file:///D:/p/a.ts","text":"x"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("typescript"));

        let mut p = json!({"textDocument":{"uri":"file:///D:/p/a.tsx","text":"x"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("typescriptreact"));

        let mut p = json!({"textDocument":{"uri":"file:///D:/p/a.rs","text":"x"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("rust"));

        // An explicit value is never overwritten.
        let mut p = json!({"textDocument":{"uri":"file:///D:/p/a.ts","languageId":"vue"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("vue"));

        // `"undefined"` is what an absent value serialises to.
        let mut p = json!({"textDocument":{"uri":"file:///D:/p/a.py","languageId":"undefined"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("python"));

        let mut p = json!({"textDocument":{"uri":"file:///D:/p/README","text":"x"}});
        ensure_language_id(&mut p, dir);
        assert_eq!(p["textDocument"]["languageId"], json!("plaintext"));
    }

    #[test]
    fn file_uri_round_trips_through_path() {
        for raw in [r"D:\Workspace\Echoo\a.ts", r"C:\a b\c.ts", "/home/u/p.ts"] {
            let uri = path_to_file_uri(raw);
            let back = file_uri_to_path(&uri).expect("local file uri");
            assert_eq!(
                back.to_str().unwrap().replace('/', "\\"),
                raw.replace('/', "\\"),
                "{raw} -> {uri}"
            );
        }
        assert!(file_uri_to_path("http://example.com/a").is_none());
    }
}

#[cfg(test)]
mod shell_detection_tests {
    use super::{is_posix_style_shell, is_powershell, shell_base_name};

    /// Regression: `C:\Program Files\Git\bin\bash.exe` used to fail every
    /// predicate, fall through to the cmd.exe branch and be invoked as
    /// `bash.exe /C "<command>"`. `-C` is not a bash flag, so EVERY command
    /// failed instantly —the whole shell appeared broken on Windows.
    #[test]
    fn windows_git_bash_is_posix_style() {
        for shell in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"D:\tools\Git\bin\bash.EXE",
        ] {
            assert_eq!(shell_base_name(shell), "bash", "for {shell}");
            assert!(is_posix_style_shell(shell), "{shell} must use -lc");
            assert!(!is_powershell(shell), "{shell} is not powershell");
        }
    }

    #[test]
    fn powershell_variants_still_detected() {
        for shell in [
            "pwsh",
            "pwsh.exe",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ] {
            assert!(is_powershell(shell), "{shell}");
        }
        assert!(!is_posix_style_shell("pwsh.exe"));
    }

    #[test]
    fn other_shells_and_extensions_normalize() {
        assert!(is_posix_style_shell("/bin/bash"));
        assert!(is_posix_style_shell("zsh"));
        assert!(is_posix_style_shell("sh.exe"));
        // A cmd-family shell must NOT be treated as posix (no `-lc`).
        assert!(!is_posix_style_shell("cmd.exe"));
        assert!(!is_posix_style_shell("C:\\Windows\\System32\\cmd.exe"));
    }
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    #[test]
    fn bash_description_tells_model_to_raise_the_timeout() {
        // The single biggest cause of "bash is unusable": a 60s default with a
        // schema that never mentioned `timeout_ms`, so the model never raised
        // it and concluded the shell was broken.
        let tool = super::BashTool::new(None, Default::default());
        let description = tool.description();
        assert!(
            description.contains("timeout_ms"),
            "description must mention timeout_ms: {description}"
        );
        let schema = tool.input_schema().to_string();
        assert!(schema.contains("timeout_ms"), "{schema}");
    }

    #[test]
    fn parse_unified_patch_keeps_last_hunk_of_each_file() {
        let patch = "\
--- a/one.txt
+++ b/one.txt
@@ -1,1 +1,1 @@
-old one
+new one
--- a/two.txt
+++ b/two.txt
@@ -1,1 +1,1 @@
-old two
+new two
";
        let files = parse_unified_patch(patch).expect("patch parsed");
        assert_eq!(files.len(), 2, "both files must be present: {files:?}");
        assert_eq!(files[0].0, "one.txt");
        assert_eq!(
            files[0].1.len(),
            1,
            "first file must keep its hunk: {:?}",
            files[0].1
        );
        assert_eq!(files[1].0, "two.txt");
        assert_eq!(files[1].1.len(), 1);
        assert_eq!(
            files[1].1[0].lines,
            vec![('-', "old two".to_string()), ('+', "new two".to_string())]
        );
    }

    #[test]
    fn parse_unified_patch_applies_multi_file_content() {
        let patch = "\
--- a/one.txt
+++ b/one.txt
@@ -1,1 +1,1 @@
-old one
+new one
--- a/two.txt
+++ b/two.txt
@@ -1,1 +1,1 @@
-old two
+new two
";
        let files = parse_unified_patch(patch).expect("patch parsed");
        let one = apply_hunks_to_content("old one\n", &files[0].1).unwrap();
        assert_eq!(one, "new one\n");
        let two = apply_hunks_to_content("old two\n", &files[1].1).unwrap();
        assert_eq!(two, "new two\n");
    }
}
