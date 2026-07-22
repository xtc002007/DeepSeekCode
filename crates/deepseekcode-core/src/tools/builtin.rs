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
use super::services::{current_services, emit_todo_updated};

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
        let content = std::fs::read_to_string(&path).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = input["limit"].as_u64().unwrap_or(2000) as usize;
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
        let content = input["content"].as_str().unwrap_or("");
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
        }
        std::fs::write(&path, content).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        let diff_stats = compute_text_diff_stats(&old, content);
        Ok(json!({
            "path": path.display().to_string(),
            "written": path.display().to_string(),
            "bytes": content.len(),
            "diff_stats": diff_stats,
        }))
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
        let old = input["old_string"].as_str().unwrap_or("");
        let new = input["new_string"].as_str().unwrap_or("");
        let content = std::fs::read_to_string(&path).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        let count = content.matches(old).count();
        if count != 1 {
            return Err(ToolFailure {
                message: format!("expected 1 match, found {count}"),
                retryable: true,
            });
        }
        let updated = content.replacen(old, new, 1);
        std::fs::write(&path, &updated).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })?;
        let diff_stats = compute_text_diff_stats(&content, &updated);
        Ok(json!({
            "path": path.display().to_string(),
            "replaced": true,
            "diff_stats": diff_stats,
        }))
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
            if cfg!(windows) {
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
        crate::process::hide_console_async(&mut cmd);

        let output = timeout(
            Duration::from_millis(timeout_ms),
            run_command_streaming(cmd, ctx),
        )
        .await
        .map_err(|_| ToolFailure {
            message: format!("command timed out after {timeout_ms}ms"),
            retryable: true,
        })??;

        let _ = crate::project_context::ensure_agents_md(&ctx.working_directory);
        Ok(output)
    }
}

async fn run_command_streaming(
    mut cmd: Command,
    ctx: &ToolContext,
) -> std::result::Result<Value, ToolFailure> {
    crate::process::hide_console_async(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| ToolFailure {
        message: e.to_string(),
        retryable: true,
    })?;
    let services = current_services();
    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !stdout.is_empty() {
                stdout.push('\n');
            }
            stdout.push_str(&line);
            if let Some(services) = &services {
                if let Some(sink) = services.progress.as_ref() {
                    let _ = sink
                        .progress(ctx.session_id, &ctx.tool_call_id, "bash", &line, "stdout")
                        .await;
                }
            }
        }
    }
    if let Some(err) = child.stderr.take() {
        let mut lines = BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !stderr.is_empty() {
                stderr.push('\n');
            }
            stderr.push_str(&line);
            if let Some(services) = &services {
                if let Some(sink) = services.progress.as_ref() {
                    let _ = sink
                        .progress(ctx.session_id, &ctx.tool_call_id, "bash", &line, "stderr")
                        .await;
                }
            }
        }
    }

    let status = child.wait().await.map_err(|e| ToolFailure {
        message: e.to_string(),
        retryable: true,
    })?;
    Ok(json!({
        "exit_code": status.code(),
        "stdout": stdout,
        "stderr": stderr,
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
        let patch = input["patch"].as_str().unwrap_or("");
        let mut applied = Vec::new();
        let mut current_file: Option<PathBuf> = None;
        let mut file_content = String::new();
        let mut old_contents: HashMap<String, String> = HashMap::new();
        let mut total_added = 0u32;
        let mut total_removed = 0u32;

        for line in patch.lines() {
            if line.starts_with("+++ ") || line.starts_with("--- ") {
                if let Some(path) = current_file.take() {
                    let old = old_contents
                        .get(&path.display().to_string())
                        .cloned()
                        .unwrap_or_default();
                    let stats = compute_text_diff_stats(&old, &file_content);
                    total_added += stats.added;
                    total_removed += stats.removed;
                    std::fs::write(&path, &file_content).map_err(|e| ToolFailure {
                        message: e.to_string(),
                        retryable: false,
                    })?;
                    applied.push(path.display().to_string());
                }
                if line.starts_with("+++ b/") {
                    let rel = line.trim_start_matches("+++ b/");
                    current_file = Some(resolve_path(ctx, rel)?);
                    if let Some(path) = &current_file {
                        let old = std::fs::read_to_string(path).unwrap_or_default();
                        old_contents.insert(path.display().to_string(), old.clone());
                        file_content = old;
                    }
                }
            } else if line.starts_with('+') && !line.starts_with("+++") {
                file_content.push_str(line.trim_start_matches('+'));
                file_content.push('\n');
            } else if line.starts_with('-') && !line.starts_with("---") {
                // skip removed lines in simplified apply
            } else if line.starts_with(' ') {
                file_content.push_str(line.trim_start_matches(' '));
                file_content.push('\n');
            }
        }
        if let Some(path) = current_file {
            let old = old_contents
                .get(&path.display().to_string())
                .cloned()
                .unwrap_or_default();
            let stats = compute_text_diff_stats(&old, &file_content);
            total_added += stats.added;
            total_removed += stats.removed;
            std::fs::write(&path, &file_content).map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            applied.push(path.display().to_string());
        }
        Ok(json!({
            "applied": !applied.is_empty(),
            "files": applied,
            "diff_stats": { "added": total_added, "removed": total_removed },
        }))
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
        let body = response.text().await.map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?;
        // Return full body; truncation is handled by ManagedToolOutput in the session runner
        // (contextual_text reads the `content` field, same as bash `stdout`).
        Ok(json!({
            "url": url,
            "status": status,
            "content": body,
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
