use deepseekcode_types::{DeepSeekCodeError, Result, SessionId};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// DeepSeek prefix-cache budget: provider-visible tool projection stays
/// within 32KiB so one huge output cannot permanently poison the prefix.
/// Full output is always retained in `.deepseekcode/tool-output/` for paged
/// recovery via the `tool_result_page` tool (`full_output_file` pointer).
pub const DEFAULT_MAX_LINES: usize = 400;
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct ToolOutputLimits {
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl Default for ToolOutputLimits {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

pub struct ManagedToolOutput {
    base_dir: PathBuf,
}

impl ManagedToolOutput {
    pub fn new(project_dir: impl AsRef<Path>) -> Self {
        let base_dir = project_dir
            .as_ref()
            .join(".deepseekcode")
            .join("tool-output");
        Self { base_dir }
    }

    pub fn project(project_dir: impl AsRef<Path>) -> Self {
        Self::new(project_dir)
    }

    pub fn store(
        &self,
        session_id: SessionId,
        tool_call_id: &str,
        tool_name: &str,
        output: &Value,
        limits: &ToolOutputLimits,
    ) -> Result<Value> {
        let start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "tool_output.store",
            session_id = %session_id,
            tool = %tool_name,
            "storage op start"
        );
        let contextual = contextual_text(output);
        if line_count(&contextual) <= limits.max_lines && contextual.len() <= limits.max_bytes {
            tracing::debug!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "tool_output.store",
                session_id = %session_id,
                tool = %tool_name,
                duration_ms = start.elapsed().as_millis() as u64,
                bytes = contextual.len(),
                "storage op ok (no truncation)"
            );
            return Ok(output.clone());
        }

        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "tool_output.store",
            session_id = %session_id,
            tool = %tool_name,
            bytes = contextual.len(),
            lines = line_count(&contextual),
            max_bytes = limits.max_bytes,
            max_lines = limits.max_lines,
            "tool output exceeds limits; truncating to file"
        );
        std::fs::create_dir_all(&self.base_dir).map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "tool_output.store",
                session_id = %session_id,
                tool = %tool_name,
                error = %e,
                "tool output directory create failed"
            );
            DeepSeekCodeError::Io(e)
        })?;

        fn sanitize_component(s: &str) -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(64)
                .collect()
        }
        let file_name = format!(
            "{}_{}_{}.json",
            session_id,
            sanitize_component(tool_call_id),
            sanitize_component(tool_name)
        );
        let path = self.base_dir.join(&file_name);
        // Containment: sanitized components cannot contain separators, but
        // verify the join stays inside base_dir regardless.
        if path.parent() != Some(self.base_dir.as_path()) {
            return Err(DeepSeekCodeError::InvalidInput(
                "invalid tool output path".into(),
            ));
        }
        let serialized = serde_json::to_string_pretty(output).map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "tool_output.store",
                session_id = %session_id,
                tool = %tool_name,
                error = %e,
                "tool output serialize failed"
            );
            DeepSeekCodeError::Serde(e)
        })?;
        std::fs::write(&path, &serialized).map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::STORAGE,
                run_id = %deepseekcode_telemetry::current_run_id(),
                operation = "tool_output.store",
                session_id = %session_id,
                tool = %tool_name,
                error = %e,
                "tool output file write failed"
            );
            DeepSeekCodeError::Io(e)
        })?;

        let marker = format!(
            "... output truncated; full content saved to {} ...",
            path.display()
        );
        let preview = bounded_preview(&contextual, &marker, limits.max_lines, limits.max_bytes);

        tracing::info!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            operation = "tool_output.store",
            session_id = %session_id,
            tool = %tool_name,
            duration_ms = start.elapsed().as_millis() as u64,
            bytes = serialized.len(),
            "storage op ok (truncated)"
        );
        Ok(project_bounded_output(output, &preview, &path))
    }
}

fn is_envelope(output: &Value) -> bool {
    output.get("title").is_some() && output.get("output").is_some()
}

fn envelope_body(output: &Value) -> Value {
    if is_envelope(output) {
        // Dirty-data tolerant: an envelope may claim `output` but carry a
        // missing/non-object value (e.g. partial writes). Never panic;
        // fall back to Null so callers degrade to empty preview.
        output.get("output").cloned().unwrap_or(Value::Null)
    } else {
        output.clone()
    }
}

fn contextual_text(output: &Value) -> String {
    let body = envelope_body(output);
    if let Some(stdout) = body.get("stdout").and_then(|v| v.as_str()) {
        let stderr = body.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stderr.is_empty() {
            return format!("{stdout}\n{stderr}");
        }
        return stdout.to_string();
    }
    if let Some(content) = body.get("content").and_then(|v| v.as_str()) {
        return content.to_string();
    }
    serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
}

fn project_bounded_output(original: &Value, preview: &str, path: &Path) -> Value {
    if is_envelope(original) {
        let inner = original.get("output").cloned().unwrap_or(Value::Null);
        let bounded_inner = project_bounded_body(&inner, preview, path);
        let mut bounded = original.clone();
        if let Some(obj) = bounded.as_object_mut() {
            obj.insert("output".into(), bounded_inner);
            obj.insert("truncated".into(), json!(true));
            obj.insert("full_output_file".into(), json!(path.display().to_string()));
        }
        return bounded;
    }
    project_bounded_body(original, preview, path)
}

fn project_bounded_body(original: &Value, preview: &str, path: &Path) -> Value {
    if original.get("stdout").is_some() {
        // `preview` is head + marker + tail; the marker line always contains
        // "output truncated". Split stdout/stderr by re-deriving from the
        // ORIGINAL value instead of splitting preview on the first newline
        // (which misplaced multi-line stdout into stderr).
        let orig_stdout = original
            .get("stdout")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let orig_stderr = original
            .get("stderr")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Keep the bounded preview as stdout and preserve stderr tail (8KiB).
        let stderr_tail = if orig_stderr.len() > 8 * 1024 {
            &orig_stderr[orig_stderr.len() - 8 * 1024..]
        } else {
            orig_stderr
        };
        let _ = orig_stdout;
        let (stdout, stderr) = (preview, stderr_tail);
        let mut bounded = original.clone();
        if let Some(obj) = bounded.as_object_mut() {
            obj.insert("stdout".into(), json!(stdout));
            obj.insert("stderr".into(), json!(stderr));
            obj.insert("truncated".into(), json!(true));
            obj.insert("full_output_file".into(), json!(path.display().to_string()));
        }
        return bounded;
    }
    if original.get("content").is_some() {
        let mut bounded = original.clone();
        if let Some(obj) = bounded.as_object_mut() {
            obj.insert("content".into(), json!(preview));
            obj.insert("truncated".into(), json!(true));
            obj.insert("full_output_file".into(), json!(path.display().to_string()));
        }
        return bounded;
    }
    json!({
        "truncated": true,
        "preview": preview,
        "full_output_file": path.display().to_string(),
        "structured": original,
    })
}

fn line_count(text: &str) -> usize {
    text.chars().filter(|c| *c == '\n').count() + if text.is_empty() { 0 } else { 1 }
}

fn take_prefix(input: &str, maximum_bytes: usize) -> String {
    let mut bytes = 0usize;
    let mut content = String::new();
    for ch in input.chars() {
        let size = ch.len_utf8();
        if bytes + size > maximum_bytes {
            break;
        }
        content.push(ch);
        bytes += size;
    }
    content
}

fn take_suffix(input: &str, maximum_bytes: usize) -> String {
    let mut bytes = 0usize;
    let mut chars: Vec<char> = Vec::new();
    for ch in input.chars().rev() {
        let size = ch.len_utf8();
        if bytes + size > maximum_bytes {
            break;
        }
        chars.push(ch);
        bytes += size;
    }
    chars.into_iter().rev().collect()
}

fn preview(text: &str, max_lines: usize, max_bytes: usize) -> (String, String) {
    let lines: Vec<&str> = text.split('\n').collect();
    let head_lines = max_lines.div_ceil(2);
    let tail_lines = max_lines / 2;
    let (head, tail) = if lines.len() <= max_lines {
        (text.to_string(), String::new())
    } else {
        (
            lines[..head_lines].join("\n"),
            if tail_lines > 0 {
                lines[lines.len() - tail_lines..].join("\n")
            } else {
                String::new()
            },
        )
    };
    let sampled = if tail.is_empty() {
        head.clone()
    } else {
        format!("{head}\n{tail}")
    };
    if sampled.len() <= max_bytes {
        return (head, tail);
    }
    let head_bytes = max_bytes.div_ceil(2);
    let tail_bytes = max_bytes / 2;
    (
        take_prefix(&head, head_bytes),
        take_suffix(&tail, tail_bytes),
    )
}

fn bounded_preview(text: &str, marker: &str, max_lines: usize, max_bytes: usize) -> String {
    let marker_only = take_prefix(marker, max_bytes);
    if max_lines <= 4 || max_bytes <= marker.len() + 4 {
        return marker_only;
    }
    let bounded = preview(
        text,
        max_lines.saturating_sub(4),
        max_bytes.saturating_sub(marker.len() + 4),
    );
    if bounded.1.is_empty() {
        format!("{}\n\n{}", bounded.0, marker)
    } else {
        format!("{}\n\n{}\n\n{}", bounded.0, marker, bounded.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_keeps_head_and_tail() {
        let text: String = (0..100)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (head, tail) = preview(&text, 10, 10_000);
        assert!(head.contains("line0"));
        assert!(tail.contains("line99"));
    }

    #[test]
    fn envelope_contextual_reads_nested_content() {
        let envelope = json!({
            "title": "Read big.txt",
            "output": { "content": "hello\nworld" },
            "metadata": {},
            "duration_ms": 1
        });
        assert_eq!(contextual_text(&envelope), "hello\nworld");
    }

    #[test]
    fn envelope_truncation_preserves_structure() {
        let big = "x".repeat(60_000);
        let envelope = json!({
            "title": "Read big.txt",
            "output": { "content": big },
            "metadata": {},
            "duration_ms": 1
        });
        let store = ManagedToolOutput::new(std::env::temp_dir());
        let limits = ToolOutputLimits {
            max_lines: 10,
            max_bytes: 100,
        };
        let bounded = store
            .store(SessionId::new(), "c1", "read", &envelope, &limits)
            .unwrap();
        assert!(bounded
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false));
        assert!(bounded.get("output").is_some());
        assert!(bounded.get("title").is_some());
    }
}
