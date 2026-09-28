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

/// Unwrap `{"raw": "..."}` envelopes, re-parse concatenated fragments, and
/// canonicalize (sorted keys) for execution. String values are preserved
/// byte-for-byte: trimming here used to corrupt `write` content and `bash`
/// commands with significant leading/trailing whitespace.
pub fn normalize_tool_arguments(args: &Value) -> Value {
    let base = if let Some(raw) = args.get("raw").and_then(|v| v.as_str()) {
        parse_tool_arguments(raw)
    } else {
        args.clone()
    };
    canonicalize_value(&base, false)
}

/// Fingerprint-only variant: additionally trims strings so `A`, `A ` and key
/// re-orderings share one loop-guard bucket. Never feed this to execution.
pub fn normalize_tool_arguments_for_fingerprint(args: &Value) -> Value {
    let base = if let Some(raw) = args.get("raw").and_then(|v| v.as_str()) {
        parse_tool_arguments(raw)
    } else {
        args.clone()
    };
    canonicalize_value(&base, true)
}

fn canonicalize_value(v: &Value, trim_strings: bool) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted: Vec<(&String, &Value)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, val) in sorted {
                out.insert(k.clone(), canonicalize_value(val, trim_strings));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|item| canonicalize_value(item, trim_strings))
                .collect(),
        ),
        Value::String(s) if trim_strings => Value::String(s.trim().to_string()),
        _ => v.clone(),
    }
}

/// Canonical fingerprint string for loop-guard: `tool:{canonical_json}`.
pub fn tool_fingerprint_canonical(tool: &str, args: &Value) -> String {
    let norm = normalize_tool_arguments_for_fingerprint(args);
    format!(
        "{tool}:{}",
        serde_json::to_string(&norm).unwrap_or_default()
    )
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

    #[test]
    fn execution_normalize_preserves_significant_whitespace() {
        let args = json!({"path": "a.txt", "content": "  indented  \n"});
        let exec = normalize_tool_arguments(&args);
        assert_eq!(exec["content"], "  indented  \n");
        let fp = normalize_tool_arguments_for_fingerprint(&args);
        assert_eq!(fp["content"], "indented");
    }
}
