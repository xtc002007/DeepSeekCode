use crate::axis::framing::sse_data_payload;
use crate::axis::{AnthropicMessagesEndpoint, ApiKeyAuth, Auth, BearerAuth, Endpoint, SseSplitter};
use crate::event::LlmEvent;
use crate::request::{LlmMessage, LlmRequest, LlmRole};
use crate::tool_call::LlmToolCall;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;

/// Build the auth closure shared by `generate` and `stream`: the official
/// Anthropic API authenticates via `x-api-key`; third-party
/// Anthropic-compatible proxies customised via `base_url` almost universally
/// expect `Authorization: Bearer` instead. (Previously this logic was
/// duplicated inline at both call sites plus a dead helper.)
fn anthropic_auth(
    base_url: Option<&str>,
    api_key: &str,
) -> impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + Clone {
    let api_key = api_key.to_string();
    let official = base_url.map(is_official_anthropic_endpoint).unwrap_or(true);
    move |b: reqwest::RequestBuilder| {
        let builder = b.header("anthropic-version", "2023-06-01");
        if official {
            ApiKeyAuth.apply(builder, &api_key)
        } else {
            BearerAuth.apply(builder, &api_key)
        }
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
    let start = std::time::Instant::now();
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref())?;
    let endpoint_host = crate::redact_endpoint_for_log(&url);
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
        "llm anthropic generate start"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm anthropic generate debug"
    );
    crate::providers::log_provider_route("anthropic", &request.model, &endpoint_host);
    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                endpoint_host = %endpoint_host,
                error = "Anthropic API key not set",
                "llm auth failed"
            );
            return Err(DeepSeekCodeError::Llm("Anthropic API key not set".into()));
        }
    };
    let body = build_body(request);

    // Route through proxy-aware fallback (auto->direct->proxy) like
    // openai/deepseek so VPN ON/OFF both work.
    let response = match crate::http_client::post_json_with_fallback_auth(
        http,
        &url,
        &api_key,
        &body,
        true,
        anthropic_auth(request.base_url.as_deref(), &api_key),
    )
    .await
    {
        Ok(r) => r,
        Err(combined) => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error_summary = %deepseekcode_telemetry::summarize_prompt(&combined, false),
                "llm anthropic generate transport failed"
            );
            crate::providers::log_provider_error("anthropic", &request.model, &combined);
            return Err(DeepSeekCodeError::Llm(combined));
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let sanitized = crate::axis::framing::sanitize_llm_error(&text);
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            endpoint_host = %endpoint_host,
            status = %status,
            duration_ms = start.elapsed().as_millis() as u64,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm anthropic generate non-success status"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            status = %status,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm anthropic generate failed"
        );
        if crate::overflow::is_context_overflow(&sanitized) {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                "llm anthropic generate context overflow"
            );
        }
        return Err(DeepSeekCodeError::Llm(sanitized));
    }

    let status = response.status();
    let value: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "llm anthropic generate JSON parse failed"
            );
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                error = %e,
                "llm anthropic generate parse failure"
            );
            return Err(DeepSeekCodeError::Llm(e.to_string()));
        }
    };

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
                _ => {
                    tracing::debug!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        provider = "anthropic",
                        model = %request.model,
                        "llm anthropic unknown content block ignored"
                    );
                }
            }
        }
    } else {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            "llm anthropic response missing content array; defaulting to empty"
        );
    }

    // Preserve usage (P0-5: previously `None`, blinding billing and
    // `/cache-status` for Anthropic). Anthropic shape:
    // `{usage:{input_tokens,output_tokens,cache_read_input_tokens,
    // cache_creation_input_tokens}}`.
    let usage = value.get("usage").map(|u| crate::request::TokenUsage {
        input_tokens: u
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        output_tokens: u
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        reasoning_tokens: None,
        cache_tokens: u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        cache_hit: u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        cache_miss: None,
        cache_write: u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
    });

    if content.len() > 1_000_000 {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            content_len = content.len(),
            "llm anthropic content overlarge; truncation risk"
        );
    }
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        duration_ms = start.elapsed().as_millis() as u64,
        status = %status,
        output_tokens = ?usage.as_ref().and_then(|u| u.output_tokens),
        tool_call_count = tool_calls.len(),
        content_len = content.len(),
        "llm anthropic generate succeeded"
    );
    Ok(crate::request::LlmResponse {
        content,
        tool_calls,
        usage,
        reasoning_content: None,
    })
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let start = std::time::Instant::now();
    let url = AnthropicMessagesEndpoint.resolve(request.base_url.as_deref())?;
    let endpoint_host = crate::redact_endpoint_for_log(&url);
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
        "llm anthropic stream start"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm anthropic stream debug"
    );
    crate::providers::log_provider_route("anthropic", &request.model, &endpoint_host);
    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                endpoint_host = %endpoint_host,
                error = "Anthropic API key not set",
                "llm auth failed"
            );
            return Err(DeepSeekCodeError::Llm("Anthropic API key not set".into()));
        }
    };
    let mut body = build_body(request);
    body["stream"] = json!(true);

    // Same fallback as generate; streams carry the 10-minute total cap from
    // the shared sender (runner idle watchdog owns sub-minute gaps).
    let response = match crate::http_client::post_json_stream_with_fallback_auth(
        http,
        &url,
        &api_key,
        &body,
        anthropic_auth(request.base_url.as_deref(), &api_key),
    )
    .await
    {
        Ok(r) => r,
        Err(combined) => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error_summary = %deepseekcode_telemetry::summarize_prompt(&combined, false),
                "llm anthropic stream transport failed"
            );
            return Err(DeepSeekCodeError::Llm(combined));
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let sanitized = crate::axis::framing::sanitize_llm_error(&text);
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            endpoint_host = %endpoint_host,
            status = %status,
            duration_ms = start.elapsed().as_millis() as u64,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm anthropic stream non-success status"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "anthropic",
            model = %request.model,
            status = %status,
            "llm anthropic stream failed"
        );
        if crate::overflow::is_context_overflow(&sanitized) {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "anthropic",
                model = %request.model,
                "llm anthropic stream context overflow"
            );
        }
        return Err(DeepSeekCodeError::Llm(sanitized));
    }
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "anthropic",
        model = %request.model,
        endpoint_host = %endpoint_host,
        duration_ms = start.elapsed().as_millis() as u64,
        status = %response.status(),
        "llm anthropic stream opened"
    );

    let byte_stream = response.bytes_stream();
    let mapped = byte_stream
        .map(|chunk| chunk.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
        .scan(
            (
                SseSplitter::new(),
                HashMap::<usize, (String, String, String)>::new(),
            ),
            |(splitter, tool_blocks), chunk| match chunk {
                Ok(bytes) => {
                    // Shared `SseSplitter`: Vec<u8> buffering, CRLF
                    // normalization, 1 MiB cap, UTF-8-boundary-safe drain.
                    let frames = splitter.push_and_split(&bytes);
                    let mut events = Vec::new();
                    for frame in &frames {
                        for line in frame.lines() {
                            let Some(data) = sse_data_payload(line) else {
                                continue;
                            };
                            let value: Value = match serde_json::from_str::<Value>(data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(
                                        target: deepseekcode_telemetry::LLM,
                                        run_id = %deepseekcode_telemetry::current_run_id(),
                                        provider = "anthropic",
                                        error = %e,
                                        data_summary = %deepseekcode_telemetry::summarize_prompt(data, false),
                                        "llm anthropic sse JSON parse failed; skipping"
                                    );
                                    tracing::error!(
                                        target: deepseekcode_telemetry::LLM,
                                        run_id = %deepseekcode_telemetry::current_run_id(),
                                        provider = "anthropic",
                                        error = %e,
                                        "llm anthropic protocol error: sse unparseable"
                                    );
                                    continue;
                                }
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

    // Honor request.max_output_tokens; `None`/`Some(0)` (legacy omit)
    // normalize to the 4096 default so `max_tokens: 0` is never sent.
    let max_tokens = request.effective_max_output_tokens().unwrap_or(4096);
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect();
    let mut body = json!({
        "model": request.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    // Omit empty tools: some gateways reject `"tools":[]`.
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
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
