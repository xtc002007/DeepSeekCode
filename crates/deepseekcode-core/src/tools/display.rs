use deepseekcode_schema::activity::DiffStats;
use deepseekcode_schema::tool::ToolResultEnvelope;
use serde_json::{json, Value};

pub fn preview_metadata_from_input(tool: &str, input: &Value) -> Value {
    let input = &deepseekcode_types::normalize_tool_arguments(input);
    match tool {
        "read" | "write" | "edit" => {
            let path = input["path"].as_str().unwrap_or("");
            let mut meta = json!({ "path": path });
            if tool == "read" {
                let offset = input["offset"].as_u64().unwrap_or(1).max(1) as u32;
                meta["line_start"] = json!(offset);
            }
            meta
        }
        "grep" => json!({
            "pattern": input["pattern"].as_str().unwrap_or(""),
            "scope": input["path"].as_str().unwrap_or("."),
        }),
        "glob" => json!({
            "pattern": input["pattern"]
                .as_str()
                .or_else(|| input["glob_pattern"].as_str())
                .unwrap_or("*"),
        }),
        "list" => json!({ "path": input["path"].as_str().unwrap_or(".") }),
        "bash" => json!({ "command": input["command"].as_str().unwrap_or("") }),
        "webfetch" => json!({ "url": input["url"].as_str().unwrap_or("") }),
        "websearch" => json!({ "query": input["query"].as_str().unwrap_or("") }),
        "skill" => json!({
            "name": input["name"].as_str().unwrap_or(""),
            "base_dir": input["base_dir"].as_str().unwrap_or(""),
        }),
        "lsp" => json!({
            "method": input["method"].as_str().unwrap_or(""),
            "target": input["target"].as_str().unwrap_or(""),
        }),
        "todowrite" | "todo_write" => json!({}),
        "task" => json!({
            "agent": input["agent"].as_str().unwrap_or(""),
            "prompt": input["prompt"].as_str().unwrap_or(""),
        }),
        "question" => json!({ "question": input["question"].as_str().unwrap_or("") }),
        _ => json!({}),
    }
}

pub fn preview_title_from_input(tool: &str, input: &Value) -> String {
    let input = &deepseekcode_types::normalize_tool_arguments(input);
    match tool {
        "read" => format!("Read {}", input["path"].as_str().unwrap_or("file")),
        "write" => format!("Write {}", input["path"].as_str().unwrap_or("file")),
        "edit" => format!("Edit {}", input["path"].as_str().unwrap_or("file")),
        "grep" => format!("Search {}", input["pattern"].as_str().unwrap_or("")),
        "glob" => format!(
            "Glob {}",
            input["pattern"]
                .as_str()
                .or_else(|| input["glob_pattern"].as_str())
                .unwrap_or("*")
        ),
        "list" => format!("List {}", input["path"].as_str().unwrap_or(".")),
        "bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            if cmd.chars().count() > 60 {
                format!("Run {}…", cmd.chars().take(60).collect::<String>())
            } else {
                format!("Run {cmd}")
            }
        }
        "apply_patch" => "Apply patch".into(),
        "webfetch" => format!("Fetch {}", input["url"].as_str().unwrap_or("")),
        "websearch" => format!("Search web {}", input["query"].as_str().unwrap_or("")),
        "skill" => format!("Skill {}", input["name"].as_str().unwrap_or("")),
        "todowrite" | "todo_write" => {
            let count = input
                .get("todos")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{count} todos")
        }
        "todoread" => "Read todos".into(),
        "task" => format!("Task {}", input["agent"].as_str().unwrap_or("agent")),
        "question" => "Ask question".into(),
        "plan_complete" | "plan.complete" => {
            format!("Complete plan {}", input["title"].as_str().unwrap_or(""))
        }
        "lsp" => "LSP".into(),
        other => other.to_string(),
    }
}

pub fn enrich_metadata(tool: &str, input: &Value, output: &Value) -> Value {
    let mut meta = preview_metadata_from_input(tool, input);
    let obj = meta.as_object_mut().unwrap();
    match tool {
        "read" => {
            if let Some(path) = output.get("path").and_then(|v| v.as_str()) {
                obj.insert("path".into(), json!(path));
            }
            if let Some(start) = output.get("line_start").and_then(|v| v.as_u64()) {
                obj.insert("line_start".into(), json!(start));
            }
            if let Some(end) = output.get("line_end").and_then(|v| v.as_u64()) {
                obj.insert("line_end".into(), json!(end));
            } else if let Some(content) = output.get("content").and_then(|v| v.as_str()) {
                let line_count = content.lines().count().max(1) as u32;
                let start = obj.get("line_start").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                obj.insert(
                    "line_end".into(),
                    json!(start + line_count.saturating_sub(1)),
                );
            }
        }
        "write" | "edit" => {
            if let Some(path) = output.get("path").and_then(|v| v.as_str()) {
                obj.insert("path".into(), json!(path));
            }
            if let Some(stats) = output.get("diff_stats") {
                obj.insert("diff_stats".into(), stats.clone());
            }
        }
        "apply_patch" => {
            if let Some(files) = output.get("files") {
                obj.insert("files".into(), files.clone());
            }
            if let Some(stats) = output.get("diff_stats") {
                obj.insert("diff_stats".into(), stats.clone());
            }
        }
        "grep" => {
            if let Some(matches) = output.get("matches").and_then(|v| v.as_array()) {
                obj.insert("match_count".into(), json!(matches.len()));
                if let Some(first) = matches.first() {
                    if let (Some(path), Some(line)) = (
                        first.get("path").and_then(|v| v.as_str()),
                        first.get("line").and_then(|v| v.as_u64()),
                    ) {
                        obj.insert("first_match".into(), json!({ "path": path, "line": line }));
                        obj.insert("first_match_path".into(), json!(path));
                        obj.insert("first_match_line".into(), json!(line));
                    }
                }
            }
        }
        "glob" => {
            if let Some(items) = output.get("files").and_then(|v| v.as_array()) {
                obj.insert("file_count".into(), json!(items.len()));
            }
        }
        "list" => {
            if let Some(items) = output.get("entries").and_then(|v| v.as_array()) {
                obj.insert("entry_count".into(), json!(items.len()));
                obj.insert("file_count".into(), json!(items.len()));
            }
        }
        "bash" => {
            if let Some(code) = output.get("exit_code") {
                obj.insert("exit_code".into(), code.clone());
            }
        }
        "webfetch" => {
            if let Some(status) = output.get("status") {
                obj.insert("status".into(), status.clone());
            }
        }
        "websearch" => {
            if let Some(results) = output.get("results").and_then(|v| v.as_array()) {
                obj.insert("result_count".into(), json!(results.len()));
                if let Some(first) = results.first() {
                    if let Some(url) = first.get("url").and_then(|v| v.as_str()) {
                        obj.insert("first_url".into(), json!(url));
                    }
                }
            }
        }
        "skill" => {
            if let Some(name) = output.get("name").and_then(|v| v.as_str()) {
                obj.insert("name".into(), json!(name));
            }
            if let Some(base_dir) = output.get("base_dir").and_then(|v| v.as_str()) {
                obj.insert("base_dir".into(), json!(base_dir));
            }
        }
        "lsp" => {
            if let Some(method) = output.get("method").and_then(|v| v.as_str()) {
                obj.insert("method".into(), json!(method));
            }
            if let Some(target) = output.get("target").and_then(|v| v.as_str()) {
                obj.insert("target".into(), json!(target));
            }
        }
        "todowrite" | "todo_write" | "todoread" => {
            if let Some(todos) = output.get("todos").and_then(|v| v.as_array()) {
                let pending = todos
                    .iter()
                    .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("pending"))
                    .count() as u32;
                let in_progress = todos
                    .iter()
                    .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
                    .count() as u32;
                obj.insert("todo_pending".into(), json!(pending));
                obj.insert("todo_in_progress".into(), json!(in_progress));
            }
        }
        "task" => {
            if let Some(sid) = output.get("session_id").and_then(|v| v.as_str()) {
                obj.insert("session_id".into(), json!(sid));
            }
        }
        _ => {}
    }
    meta
}

pub fn build_tool_result(
    tool: &str,
    input: &Value,
    output: Value,
    duration_ms: u64,
    hook_title: Option<String>,
    hook_metadata: Value,
) -> ToolResultEnvelope {
    let title = hook_title.unwrap_or_else(|| preview_title_from_input(tool, input));
    let mut metadata = enrich_metadata(tool, input, &output);
    if let (Some(hook_obj), Some(meta_obj)) = (hook_metadata.as_object(), metadata.as_object_mut())
    {
        for (k, v) in hook_obj {
            meta_obj.insert(k.clone(), v.clone());
        }
    }
    ToolResultEnvelope {
        title,
        output,
        metadata,
        duration_ms,
    }
}

pub fn line_diff_stats(old: &str, new: &str) -> DiffStats {
    let old_lines = old.lines().count();
    let new_lines = new.lines().count();
    if new_lines >= old_lines {
        DiffStats {
            added: (new_lines - old_lines) as u32 + if old != new { 1 } else { 0 },
            removed: if old != new && old_lines > 0 { 1 } else { 0 },
        }
    } else {
        DiffStats {
            added: 0,
            removed: (old_lines - new_lines) as u32,
        }
    }
}

/// Simple line-based diff stats for write/edit.
pub fn compute_text_diff_stats(old: &str, new: &str) -> DiffStats {
    let old_set: Vec<&str> = old.lines().collect();
    let new_set: Vec<&str> = new.lines().collect();
    let mut added = 0u32;
    let mut removed = 0u32;
    let max_len = old_set.len().max(new_set.len());
    for i in 0..max_len {
        match (old_set.get(i), new_set.get(i)) {
            (Some(a), Some(b)) if a != b => {
                added += 1;
                removed += 1;
            }
            (Some(_), None) => removed += 1,
            (None, Some(_)) => added += 1,
            _ => {}
        }
    }
    DiffStats { added, removed }
}
