pub trait Endpoint {
    fn resolve(&self, base_url: Option<&str>) -> String;
}

pub struct OpenAiChatEndpoint;

fn join_endpoint(base: &str, suffix: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return suffix.trim_start_matches('/').to_string();
    }
    // Avoid double suffix when the base already ends with it.
    if trimmed.ends_with(suffix) || trimmed.ends_with(&format!("{suffix}/")) {
        return trimmed.trim_end_matches('/').to_string();
    }
    format!("{trimmed}{suffix}")
}

fn validate_base_url(base: Option<&str>) -> Option<String> {
    let raw = base?.trim();
    if raw.is_empty() || raw.len() > 512 || raw.contains([' ', '\0', '\n', '\r']) {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return None;
    }
    Some(raw.trim_end_matches('/').to_string())
}

impl Endpoint for OpenAiChatEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        match validate_base_url(base_url) {
            Some(valid) => join_endpoint(&valid, "/chat/completions"),
            None => "https://api.openai.com/v1/chat/completions".to_string(),
        }
    }
}

pub struct AnthropicMessagesEndpoint;

impl Endpoint for AnthropicMessagesEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        match validate_base_url(base_url) {
            Some(valid) => join_endpoint(&valid, "/messages"),
            None => "https://api.anthropic.com/v1/messages".to_string(),
        }
    }
}
