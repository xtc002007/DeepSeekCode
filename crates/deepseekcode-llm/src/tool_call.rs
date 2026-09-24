use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl LlmToolCall {
    /// Strict arguments parsing: returns the parsed value or a typed error
    /// instead of silently substituting `{}` (P0-5). Callers decide whether
    /// to reject or repair; silent `{}` previously masked provider bugs.
    pub fn parse_arguments_strict(raw: &str) -> Result<Value, String> {
        if raw.trim().is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        serde_json::from_str(raw).map_err(|e| format!("invalid tool arguments JSON: {e}"))
    }
}
