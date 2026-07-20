use serde_json::Value;

pub fn estimate_str(value: &str) -> u32 {
    (value.len() / 4).max(1) as u32
}

pub fn estimate_json(value: &Value) -> u32 {
    serde_json::to_string(value)
        .map(|s| estimate_str(&s))
        .unwrap_or(1)
}
