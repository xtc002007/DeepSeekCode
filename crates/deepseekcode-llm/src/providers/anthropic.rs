use crate::axis::framing::sse_data_payload;
use crate::axis::{AnthropicMessagesEndpoint, ApiKeyAuth, Auth, BearerAuth, Endpoint};
use crate::event::LlmEvent;
use crate::request::{LlmMessage, LlmRequest, LlmRole};
use crate::tool_call::LlmToolCall;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

/// Non-streaming requests have no natural end, so cap them outright.
const GENERATE_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// The official Anthropic API authenticates via `x-api-key`; third-party
/// Anthropic-compatible proxies customised via `base_url` almost universally
/// expect `Authorization: Bearer` instead.
fn authenticated_builder(
    http: &Client,
    url: &str,
    base_url: Option<&str>,
    api_key: &str,
) -> reqwest::RequestBuilder {
    let builder = http.post(url).header("anthropic-version", "2023-06-01");
    if base_url.map(is_official_anthropic_endpoint).unwrap_or(true) {
        ApiKeyAuth.apply(builder, api_key)
    } else {
        BearerAuth.apply(builder, api_key)
    }
}

fn is_official_anthropic_endpoint(base_url: &str) -> bool {
    let host = base_url
        .to_lowercase()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string();
    host == "api.anthropic.com"
        || host.starts_with("api.anthropic.com/")
        || host.starts_with("api.anthropic.com:")
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<crate::request::LlmResponse> {
    let api_key =
        api_key.ok_or_else(|| DeepSeekCodeError::Llm("Anthropic API key not set".into()))?;
    let body = build_body(request);
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref());

    let response = authenticated_builder(&http, &url, request.base_url.as_deref(), &api_key)
        .timeout(GENERATE_TOTAL_TIMEOUT)
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
    let api_key =
        api_key.ok_or_else(|| DeepSeekCodeError::Llm("Anthropic API key not set".into()))?;
    let mut body = build_body(request);
    body["stream"] = json!(true);
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref());

    let response = authenticated_builder(&http, &url, request.base_url.as_deref(), &api_key)
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
            |(buffer, tool_blocks), chunk| match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    let mut events = Vec::new();
                    while let Some(pos) = buffer.find("\n\n") {
                        let frame = buffer.drain(..=pos).collect::<String>();
                        for line in frame.lines() {
                            let Some(data) = sse_data_payload(line) else {
                                continue;
                            };
                            let Ok(value) = serde_json::from_str::<Value>(data) else {
                                continue;
                            };
                            match value["type"].as_str() {
                                Some("content_block_start") => {
                                    if let Some(block) = value.get("content_block") {
                                        if block["type"].as_str() == Some("tool_use") {
                                            let index =
                                                value["index"].as_u64().unwrap_or(0) as usize;
                                            tool_blocks.insert(
                                                index,
                                                (
                                                    block["id"].as_str().unwrap_or("").to_string(),
                                                    block["name"]
                                                        .as_str()
                                                        .unwrap_or("")
                                                        .to_string(),
                                                    String::new(),
                                                ),
                                            );
                                        }
                                    }
                                }
                                Some("content_block_delta") => {
                                    let delta = &value["delta"];
                                    match delta["type"].as_str() {
                                        Some("text_delta") => {
                                            if let Some(text) = delta["text"].as_str() {
                                                events.push(Ok(LlmEvent::TextDelta(
                                                    text.to_string(),
                                                )));
                                            }
                                        }
                                        Some("input_json_delta") => {
                                            let index =
                                                value["index"].as_u64().unwrap_or(0) as usize;
                                            if let Some(partial) = delta["partial_json"].as_str() {
                                                if let Some(entry) = tool_blocks.get_mut(&index) {
                                                    entry.2.push_str(partial);
                                                    events.push(Ok(LlmEvent::ToolCallDelta {
                                                        index,
                                                        id: entry.0.clone(),
                                                        name: entry.1.clone(),
                                                        arguments: partial.to_string(),
                                                    }));
                                                }
                                            }
                                        }
                                        _ => {
                                            if let Some(text) = delta["text"].as_str() {
                                                events.push(Ok(LlmEvent::TextDelta(
                                                    text.to_string(),
                                                )));
                                            }
                                        }
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
            },
        )
        .flat_map(futures::stream::iter);

    Ok(Box::pin(mapped))
}

fn build_body(request: &LlmRequest) -> Value {
    let mut messages = Vec::new();
    let mut pending_tool_results = Vec::new();

    for message in &request.messages {
        match message.role {
            LlmRole::System => {
                // The system prompt travels via `request.system` (body["system"]);
                // re-emitting it as a user message would duplicate it in the
                // provider request.
            }
            LlmRole::User => {
                flush_tool_results(&mut messages, &mut pending_tool_results);
                messages.push(json!({
                    "role": "user",
                    "content": message_content_blocks(message),
                }));
            }
            LlmRole::Assistant => {
                flush_tool_results(&mut messages, &mut pending_tool_results);
                messages.push(json!({
                    "role": "assistant",
                    "content": assistant_content_blocks(message),
                }));
            }
            LlmRole::Tool => {
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.as_deref().unwrap_or(""),
                    "content": message.content,
                }));
            }
        }
    }
    flush_tool_results(&mut messages, &mut pending_tool_results);

    let mut body = json!({
        "model": request.model,
        "max_tokens": 4096,
        "messages": messages,
        "tools": request.tools.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.input_schema,
        })).collect::<Vec<_>>(),
    });
    if !request.system.is_empty() {
        body["system"] = json!(request.system);
    }
    body
}

fn flush_tool_results(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": std::mem::take(pending),
    }));
}

fn message_content_blocks(message: &LlmMessage) -> Value {
    if message.content.is_empty() {
        json!([])
    } else {
        json!([{
            "type": "text",
            "text": message.content,
        }])
    }
}

fn assistant_content_blocks(message: &LlmMessage) -> Value {
    let mut blocks = Vec::new();
    if !message.content.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }
    for call in &message.tool_calls {
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.arguments,
        }));
    }
    if blocks.is_empty() {
        json!([])
    } else {
        Value::Array(blocks)
    }
}
