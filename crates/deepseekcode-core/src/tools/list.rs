use async_trait::async_trait;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};

use super::builtin::resolve_path;
use super::registry::Tool;

pub struct ListTool;

#[async_trait]
impl Tool for ListTool {
    fn name(&self) -> &str {
        "list"
    }
    fn description(&self) -> &str {
        "List files and directories at a path"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        })
    }
    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let path = resolve_path(ctx, input["path"].as_str().unwrap_or("."))?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: false,
        })? {
            let entry = entry.map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            let ft = entry.file_type().map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: false,
            })?;
            entries.push(json!({
                "name": entry.file_name().to_string_lossy().to_string(),
                "path": entry.path().display().to_string(),
                "is_dir": ft.is_dir(),
            }));
        }
        Ok(json!({ "entries": entries, "path": path.display().to_string() }))
    }
}
