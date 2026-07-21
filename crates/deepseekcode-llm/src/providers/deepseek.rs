//! DeepSeek API — OpenAI-compatible Chat Completions.
//! Docs: https://api-docs.deepseek.com/
//! V4 Flash model id: `deepseek-v4-flash`

use crate::event::LlmEvent;
use crate::providers::openai;
use crate::request::LlmRequest;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";

fn resolve_base_url(base_url: Option<&str>) -> &str {
    base_url.unwrap_or(DEFAULT_BASE_URL)
}

fn deepseek_extras(request: &LlmRequest) -> Value {
    // Non-thinking mode by default for agent/tool loops; enable via model suffix if needed.
    if request.model.contains("thinking") {
        json!({ "thinking": { "type": "enabled" } })
    } else {
        json!({ "thinking": { "type": "disabled" } })
    }
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<crate::request::LlmResponse> {
    let api_key = api_key.ok_or_else(|| {
        DeepSeekCodeError::Llm("DeepSeek API key not set (DEEPSEEK_API_KEY)".into())
    })?;
    let body = openai::build_chat_body(request, false, Some(&deepseek_extras(request)));

    let response = http
        .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(DeepSeekCodeError::Llm(text));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    openai::parse_message_response(&value)
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let api_key = api_key.ok_or_else(|| {
        DeepSeekCodeError::Llm("DeepSeek API key not set (DEEPSEEK_API_KEY)".into())
    })?;
    let body = openai::build_chat_body(request, true, Some(&deepseek_extras(request)));

    let response = http
        .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(DeepSeekCodeError::Llm(text));
    }

    let byte_stream = response.bytes_stream();
    let mapped = byte_stream
        .map(|chunk| chunk.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
        .scan(
            (
                String::new(),
                HashMap::<usize, (String, String, String)>::new(),
            ),
            |state, chunk| {
                let (buffer, tool_acc) = state;
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let mut events = Vec::new();
                        while let Some(pos) = buffer.find("\n\n") {
                            let frame = buffer.drain(..=pos).collect::<String>();
                            events.extend(openai::parse_sse_frame(&frame, tool_acc));
                        }
                        futures::future::ready(Some(events))
                    }
                    Err(e) => futures::future::ready(Some(vec![Err(e)])),
                }
            },
        )
        .flat_map(futures::stream::iter);

    Ok(Box::pin(mapped))
}
