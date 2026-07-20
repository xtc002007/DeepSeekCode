pub trait Endpoint {
    fn resolve(&self, base_url: Option<&str>) -> String;
}

pub struct OpenAiChatEndpoint;

impl Endpoint for OpenAiChatEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        base_url
            .map(|u| format!("{}/chat/completions", u.trim_end_matches('/')))
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string())
    }
}

pub struct AnthropicMessagesEndpoint;

impl Endpoint for AnthropicMessagesEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        base_url
            .map(|u| format!("{}/messages", u.trim_end_matches('/')))
            .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string())
    }
}
