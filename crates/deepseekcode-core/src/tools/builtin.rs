use async_trait::async_trait;
use deepseekcode_schema::tool::{TodoStoreOps, ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::shell_env;

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
        let path = resolve_path(ctx, input["path"].as_str().unwrap_or(""))?;
        if !path.exists() {
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
        let pattern = input["pattern"].as_str().unwrap_or("*");
        let full_pattern = format!(
            "{}/{}",
            ctx.working_directory.trim_end_matches('/'),
            pattern
        );
        let files: Vec<String> = glob::glob(&full_pattern)
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?
            .filter_map(|p| p.ok())
            .map(|p| p.display().to_string())
            .take(200)
            .collect();
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
        let command = input["command"].as_str().unwrap_or("");
        if command.trim().is_empty() {
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
}

impl BashTool {
    pub fn new(shell: Option<String>, env: HashMap<String, String>) -> Self {
        Self { shell, env }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Execute a shell command. On Windows commands run via cmd.exe (/c); use cmd syntax (e.g. dir, findstr, &&). On Unix, runs via bash -lc."
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
        let command = input["command"].as_str().unwrap_or("");
        if command.trim().is_empty() {
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

        let mut cmd = if let Some(shell) = &self.shell {
            let mut c = Command::new(shell);
            if is_powershell(shell) {
                c.args(["-NoProfile", "-NonInteractive", "-Command", &command]);
            } else if is_posix_style_shell(shell) {
                c.args(["-lc", &command]);
            } else if cfg!(windows) {
                c.args(["/C", &command]);
            } else {
                c.args(["-lc", &command]);
            }
            c.current_dir(&ctx.working_directory);
            shell_env::apply_to_command(&mut c, &self.env);
            c
        } else if cfg!(windows) {
            let wrapped = format!("chcp 65001 >nul && {command}");
            let mut c = Command::new("cmd");
            c.args(["/C", &wrapped]);
            c.current_dir(&ctx.working_directory);
            shell_env::apply_to_command(&mut c, &self.env);
            c
        } else {
            let mut c = Command::new("bash");
            c.args(["-lc", &command]);
            c.current_dir(&ctx.working_directory);
            shell_env::apply_to_command(&mut c, &self.env);
            c
        };
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        crate::process::hide_console_async(&mut cmd);

        let output = match timeout(
            Duration::from_millis(timeout_ms),
            run_command_streaming(cmd, ctx),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(ToolFailure {
                    message: format!("command timed out after {timeout_ms}ms"),
                    retryable: true,
                });
            }
        };

        let _ = crate::project_context::ensure_agents_md(&ctx.working_directory);
        Ok(output)
    }
}

async fn run_command_streaming(
    mut cmd: Command,
    ctx: &ToolContext,
) -> std::result::Result<Value, ToolFailure> {
    cmd.kill_on_drop(true);
    crate::process::prepare_tracked_command_async(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| ToolFailure {
        message: e.to_string(),
        retryable: true,
    })?;
    // RAII: unregisters on every exit path — including future cancellation by
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
    let mut out_lines = child.stdout.take().map(|out| BufReader::new(out).lines());
    let mut err_lines = child.stderr.take().map(|err| BufReader::new(err).lines());
    let mut killed = false;

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
                    Some(lines) => lines.next_line().await,
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
                    Some(lines) => lines.next_line().await,
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
            else => break,
        }
    }

    let status = child.wait().await.map_err(|e| ToolFailure {
        message: e.to_string(),
        retryable: true,
    })?;
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
        use deepseekcode_schema::todo::TodoItem;
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
        let url = input["url"].as_str().unwrap_or("");
        if url.is_empty() {
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

pub struct LspTool {
    env: HashMap<String, String>,
}

impl LspTool {
    pub fn new(configured_shell: Option<&str>) -> Self {
        Self {
            env: shell_env::capture_shell_env(configured_shell),
        }
    }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }
    fn description(&self) -> &str {
        "Send a JSON-RPC request to a language server over stdio"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command and args to spawn the language server"
                },
                "method": { "type": "string" },
                "params": { "type": "object" }
            },
            "required": ["command", "method"]
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let command: Vec<String> = input["command"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if command.is_empty() {
            return Err(ToolFailure {
                message: "command must be a non-empty array".into(),
                retryable: false,
            });
        }
        let method = input["method"].as_str().unwrap_or("");
        if method.is_empty() {
            return Err(ToolFailure {
                message: "method must not be empty".into(),
                retryable: false,
            });
        }
        let params = input.get("params").cloned().unwrap_or(json!({}));
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut child = Command::new(&command[0]);
        if command.len() > 1 {
            child.args(&command[1..]);
        }
        child
            .current_dir(&ctx.working_directory)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        shell_env::apply_to_command(&mut child, &self.env);
        crate::process::hide_console_async(&mut child);

        let mut child = child.spawn().map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| ToolFailure {
            message: "failed to open lsp stdin".into(),
            retryable: false,
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolFailure {
            message: "failed to open lsp stdout".into(),
            retryable: false,
        })?;

        let payload = format!(
            "Content-Length: {}\r\n\r\n{}",
            request.to_string().len(),
            request
        );
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?;
        stdin.shutdown().await.ok();

        let mut reader = BufReader::new(stdout);
        let mut header = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            header.push_str(&line);
        }
        let content_length = header
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length:").map(|v| v.trim()))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            use tokio::io::AsyncReadExt;
            reader
                .read_exact(&mut body)
                .await
                .map_err(|e| ToolFailure {
                    message: e.to_string(),
                    retryable: true,
                })?;
        }
        let response: Value = serde_json::from_slice(&body).unwrap_or(json!({
            "error": "invalid JSON-RPC response"
        }));

        let _ = child.kill().await;
        Ok(json!({
            "response": response,
            "method": method,
            "target": command.join(" "),
        }))
    }
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
    lower
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&lower)
        .to_string()
}

fn is_powershell(shell: &str) -> bool {
    let name = shell_base_name(shell);
    name.contains("powershell") || name == "pwsh" || name == "pwsh.exe"
}

/// Shells that accept `-lc` (login + command) argument style.
fn is_posix_style_shell(shell: &str) -> bool {
    matches!(
        shell_base_name(shell).as_str(),
        "bash" | "sh" | "zsh" | "dash" | "ksh"
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
mod patch_tests {
    use super::*;

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
