use crate::event::LlmEvent;
use crate::providers::openai;
use crate::request::LlmRequest;
use deepseekcode_types::Result;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

fn require_key(api_key: Option<String>) -> Result<String> {
    api_key.filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        deepseekcode_types::DeepSeekCodeError::Llm("OpenRouter API key not set".into())
    })
}

fn effective_base(request: &LlmRequest) -> &str {
    request
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://openrouter.ai/api/v1")
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<crate::request::LlmResponse> {
    let api_key = require_key(api_key)?;
    request.validate()?;
    openai::generate(http, request, Some(api_key), Some(effective_base(request))).await
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let api_key = require_key(api_key)?;
    request.validate()?;
    openai::stream(http, request, Some(api_key), Some(effective_base(request))).await
}
