use deepseekcode_types::{DeepSeekCodeError, Result, SessionId};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_LINES: usize = 2_000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

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
        let contextual = contextual_text(output);
        if line_count(&contextual) <= limits.max_lines && contextual.len() <= limits.max_bytes {
            return Ok(output.clone());
        }

        std::fs::create_dir_all(&self.base_dir).map_err(DeepSeekCodeError::Io)?;

        let file_name = format!("{}_{}_{}.json", session_id, tool_call_id, tool_name);
        let path = self.base_dir.join(&file_name);
        let serialized = serde_json::to_string_pretty(output).map_err(DeepSeekCodeError::Serde)?;
        std::fs::write(&path, &serialized).map_err(DeepSeekCodeError::Io)?;

        let marker = format!(
            "... output truncated; full content saved to {} ...",
            path.display()
        );
        let preview = bounded_preview(&contextual, &marker, limits.max_lines, limits.max_bytes);

        Ok(project_bounded_output(output, &preview, &path))
    }
}

fn is_envelope(output: &Value) -> bool {
    output.get("title").is_some() && output.get("output").is_some()
}

fn envelope_body(output: &Value) -> &Value {
    if is_envelope(output) {
        output.get("output").unwrap()
    } else {
        output
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
    serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string())
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
        let lines: Vec<&str> = preview.splitn(2, '\n').collect();
        let (stdout, stderr) = if lines.len() == 2 {
            (lines[0], lines[1])
        } else {
            (preview, "")
        };
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
