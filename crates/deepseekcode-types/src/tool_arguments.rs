use serde_json::{json, Value};

/// Parse streamed tool-call argument JSON. Providers may concatenate duplicate
/// JSON objects when SSE finish frames replay accumulated arguments.
pub fn parse_tool_arguments(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return value;
    }

    let mut last_valid = None;
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let begin = i;
        let mut depth = 0;
        let mut j = i;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let slice = &trimmed[begin..=j];
                        if let Ok(value) = serde_json::from_str::<Value>(slice) {
                            last_valid = Some(value);
                        }
                        i = j + 1;
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
    }

    last_valid.unwrap_or_else(|| json!({ "raw": trimmed }))
}

/// Unwrap `{"raw": "..."}` envelopes and re-parse concatenated JSON fragments.
pub fn normalize_tool_arguments(args: &Value) -> Value {
    if let Some(raw) = args.get("raw").and_then(|v| v.as_str()) {
        return parse_tool_arguments(raw);
    }
    args.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duplicated_glob_pattern() {
        let raw = r#"{"pattern": "*"}{"pattern": "*"}"#;
        let parsed = parse_tool_arguments(raw);
        assert_eq!(parsed["pattern"], "*");
    }

    #[test]
    fn parses_duplicated_bash_command() {
        let raw = r#"{"command": "dir /b \"C:\\projects\\demo\\crates\" 2>nul || echo \"Directory not found\""}{"command": "dir /b \"C:\\projects\\demo\\crates\" 2>nul || echo \"Directory not found\""}"#;
        let parsed = parse_tool_arguments(raw);
        assert!(parsed["command"].as_str().unwrap().contains("dir /b"));
    }

    #[test]
    fn normalizes_raw_envelope() {
        let wrapped = json!({
            "raw": r#"{"pattern": "**/*.py"}{"pattern": "**/*.py"}"#
        });
        let parsed = normalize_tool_arguments(&wrapped);
        assert_eq!(parsed["pattern"], "**/*.py");
    }
}
