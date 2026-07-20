use crate::event::LlmEvent;
use crate::providers::openai;
use crate::request::LlmRequest;
use deepseekcode_types::Result;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<crate::request::LlmResponse> {
    let api_key = api_key.ok_or_else(|| {
        deepseekcode_types::DeepSeekCodeError::Llm("OpenRouter API key not set".into())
    })?;
    openai::generate(
        http,
        request,
        Some(api_key),
        Some("https://openrouter.ai/api/v1"),
    )
    .await
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let api_key = api_key.ok_or_else(|| {
        deepseekcode_types::DeepSeekCodeError::Llm("OpenRouter API key not set".into())
    })?;
    openai::stream(
        http,
        request,
        Some(api_key),
        Some("https://openrouter.ai/api/v1"),
    )
    .await
}
