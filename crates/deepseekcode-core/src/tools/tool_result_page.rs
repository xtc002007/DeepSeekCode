//! Issue-10: paged recovery for truncated tool output.
//!
//! Large outputs are stored in full under `.deepseekcode/tool-output/` while
//! the provider-visible prefix carries only a ≤32KiB projection plus a
//! `full_output_file` pointer. This stable tool reads one UTF-8-safe page
//! (default 16KiB, max 24KiB) so the model recovers exactly the bytes it
//! needs by appending pages to the tail — history prefixes are never rewritten.

use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const PAGE_DEFAULT_BYTES: usize = 16 * 1024;
pub const PAGE_MAX_BYTES: usize = 24 * 1024;

pub struct ToolResultPageTool;

#[async_trait]
impl super::registry::Tool for ToolResultPageTool {
    fn name(&self) -> &str {
        "tool_result_page"
    }

    fn description(&self) -> &str {
        // NOTE: byte bounds must match PAGE_DEFAULT_BYTES / PAGE_MAX_BYTES;
        // enforced by `schema_bounds_match_constants` below.
        "Read one page of a truncated tool output file (full_output_file). Args: path (required), offset (bytes, default 0), limit (bytes, default 16384, max 24576). Returns {path,offset,next_offset,total_bytes,sha256,complete,out_of_range,page}."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": PAGE_MAX_BYTES}
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
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %ctx.session_id,
            tool = "tool_result_page",
            path = %input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
            "tool_result_page started"
        );
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolFailure {
                message: "missing path (use full_output_file from truncated output)".into(),
                retryable: false,
            })?;
        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(PAGE_DEFAULT_BYTES as u64) as usize;
        let limit = limit.clamp(1, PAGE_MAX_BYTES);

        // Confine reads to the project directory (no prefix churn, no escape).
        let workdir = std::path::Path::new(&ctx.working_directory);
        let candidate = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            workdir.join(path)
        };
        let canonical_workdir = workdir
            .canonicalize()
            .unwrap_or_else(|_| workdir.to_path_buf());
        let canonical_file = candidate.canonicalize().map_err(|_| ToolFailure {
            message: format!("output file not found: {path}"),
            retryable: false,
        })?;
        if !canonical_file.starts_with(&canonical_workdir) {
            return Err(ToolFailure {
                message: "path outside project directory".into(),
                retryable: false,
            });
        }

        let bytes = std::fs::read(&canonical_file).map_err(|e| ToolFailure {
            message: format!("read failed: {e}"),
            retryable: true,
        })?;
        let total = bytes.len();
        if offset >= total {
            // `out_of_range` distinguishes "past the end" from "empty file"
            // (both yield an empty page with `complete: true`).
            return Ok(json!({
                "path": path,
                "offset": offset,
                "next_offset": total,
                "total_bytes": total,
                "sha256": format!("{:x}", Sha256::digest(&bytes)),
                "complete": true,
                "out_of_range": offset > total,
                "page": "",
            }));
        }
        let mut end = (offset + limit).min(total);
        // Snap end to a UTF-8 boundary (file content is JSON/text).
        while end < total && !is_boundary(&bytes, end) {
            end -= 1;
        }
        let mut start = offset;
        while start < end && !is_boundary(&bytes, start) {
            start += 1;
        }
        let page = String::from_utf8_lossy(&bytes[start..end]).to_string();
        Ok(json!({
            "path": path,
            "offset": start,
            "next_offset": end,
            "total_bytes": total,
            "sha256": format!("{:x}", Sha256::digest(&bytes)),
            "complete": end >= total,
            "page": page,
        }))
    }
}

fn is_boundary(bytes: &[u8], idx: usize) -> bool {
    if idx >= bytes.len() {
        return true;
    }
    // UTF-8 continuation bytes are 10xxxxxx.
    bytes[idx] & 0b1100_0000 != 0b1000_0000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::Tool;
    use deepseekcode_types::{MessageId, SessionId};

    #[tokio::test]
    async fn pages_with_utf8_safe_bounds() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("out.json");
        std::fs::write(&file, "a".repeat(100_000)).unwrap();
        let tool = ToolResultPageTool;
        let ctx = ToolContext {
            session_id: SessionId::new(),
            agent: "build".into(),
            assistant_message_id: MessageId::new(),
            tool_call_id: deepseekcode_types::ToolCallId::new(),
            working_directory: dir.path().to_string_lossy().to_string(),
            extras: Default::default(),
        };
        let out = tool
            .execute(
                json!({"path": file.to_string_lossy(), "offset": 0, "limit": 16384}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out["total_bytes"], 100_000);
        assert_eq!(out["complete"], false);
        assert!((out["next_offset"].as_u64().unwrap() as usize) <= 16384);
    }

    #[test]
    fn schema_bounds_match_constants() {
        // The human-readable description and JSON schema must track
        // PAGE_DEFAULT_BYTES / PAGE_MAX_BYTES (previously hardcoded 24576 in
        // three places and already drifted once).
        let tool = ToolResultPageTool;
        let schema = tool.input_schema();
        assert_eq!(
            schema["properties"]["limit"]["maximum"].as_u64().unwrap(),
            PAGE_MAX_BYTES as u64
        );
        assert!(tool.description().contains(&PAGE_MAX_BYTES.to_string()));
        assert!(tool.description().contains(&PAGE_DEFAULT_BYTES.to_string()));
    }
}
