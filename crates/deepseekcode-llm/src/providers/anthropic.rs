use crate::axis::{AnthropicMessagesEndpoint, Auth, BearerAuth, Endpoint};
use crate::event::LlmEvent;
use crate::request::LlmRequest;
use crate::tool_call::LlmToolCall;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<crate::request::LlmResponse> {
    let api_key = api_key.ok_or_else(|| DeepSeekCodeError::Llm("Anthropic API key not set".into()))?;
    let body = build_body(request);
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref());

    let response = BearerAuth
        .apply(
            http.post(&url).header("anthropic-version", "2023-06-01"),
            &api_key,
        )
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

    let mut content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(parts) = value["content"].as_array() {
        for part in parts {
            match part["type"].as_str() {
                Some("text") => content.push_str(part["text"].as_str().unwrap_or("")),
                Some("tool_use") => tool_calls.push(LlmToolCall {
                    id: part["id"].as_str().unwrap_or("").to_string(),
                    name: part["name"].as_str().unwrap_or("").to_string(),
                    arguments: part["input"].clone(),
                }),
                _ => {}
            }
        }
    }

    Ok(crate::request::LlmResponse {
        content,
        tool_calls,
        usage: None,
    })
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let api_key = api_key.ok_or_else(|| DeepSeekCodeError::Llm("Anthropic API key not set".into()))?;
    let mut body = build_body(request);
    body["stream"] = json!(true);
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref());

    let response = BearerAuth
        .apply(
            http.post(&url).header("anthropic-version", "2023-06-01"),
            &api_key,
        )
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
        .scan(String::new(), |buffer, chunk| {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    let mut events = Vec::new();
                    while let Some(pos) = buffer.find("\n\n") {
                        let frame = buffer.drain(..=pos).collect::<String>();
                        for line in frame.lines() {
                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };
                            let Ok(value) = serde_json::from_str::<Value>(data) else {
                                continue;
                            };
                            match value["type"].as_str() {
                                Some("content_block_delta") => {
                                    if let Some(text) = value["delta"]["text"].as_str() {
                                        events.push(Ok(LlmEvent::TextDelta(text.to_string())));
                                    }
                                }
                                Some("message_stop") => {
                                    events.push(Ok(LlmEvent::Completed));
                                }
                                _ => {}
                            }
                        }
                    }
                    futures::future::ready(Some(events))
                }
                Err(e) => futures::future::ready(Some(vec![Err(e)])),
            }
        })
        .flat_map(futures::stream::iter);

    Ok(Box::pin(mapped))
}

fn build_body(request: &LlmRequest) -> Value {
    json!({
        "model": request.model,
        "max_tokens": 4096,
        "system": request.system,
        "messages": request.messages.iter().map(|m| json!({
            "role": match m.role {
                crate::request::LlmRole::User => "user",
                crate::request::LlmRole::Assistant => "assistant",
                _ => "user",
            },
            "content": m.content,
        })).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.input_schema,
        })).collect::<Vec<_>>(),
    })
}
