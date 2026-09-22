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

/// Issue-2: real effort mapping. `disabled` (default) keeps agent/tool
/// loops cheap and prefix-stable; `enabled`/`high`/`max` opt into thinking.
/// The result is part of the cache lineage — changing effort intentionally
/// starts a new prefix.
pub fn normalize_deepseek_thinking(model: &str, effort: Option<&str>) -> &'static str {
    // Delegates to the shared normalizer (Issues 2+4): DeepSeek provider
    // paths are always DeepSeek, so `is_deepseek=true`.
    crate::request::normalize_thinking(model, effort, true)
}

/// Issue-4: `stream_options.include_usage` is only safe on the official
/// endpoint. Third-party DeepSeek-compatible gateways may reject the unknown
/// field with 400. Effective URL = explicit override → request → default.
pub fn is_official_deepseek_endpoint(
    request_base: Option<&str>,
    override_base: Option<&str>,
) -> bool {
    let effective = override_base.or(request_base).unwrap_or(DEFAULT_BASE_URL);
    effective.contains("deepseek.com")
}

fn deepseek_extras(request: &LlmRequest, stream: bool, override_base: Option<&str>) -> Value {
    // Non-thinking mode by default for agent/tool loops; enable via model
    // suffix or explicit `effort` if needed. The object stays byte-stable
    // across turns so it never breaks the automatic prefix cache.
    // `stream_options.include_usage` is required to receive
    // `prompt_cache_hit/miss_tokens` on streams — without it cache hit-rate
    // is unobservable and every turn is billed blind.
    // See Issue-4: `stream_options` is only attached for the official
    // endpoint (gated inline below via `is_official_deepseek_endpoint`).
    let thinking_type = normalize_deepseek_thinking(&request.model, request.effort.as_deref());
    let thinking = json!({ "type": thinking_type });
    // Issue-4 gate: only official understands `stream_options`.
    let official = is_official_deepseek_endpoint(request.base_url.as_deref(), override_base);
    if stream && official {
        json!({
            "thinking": thinking,
            "stream_options": { "include_usage": true },
        })
    } else {
        json!({ "thinking": thinking })
    }
    // NOTE: `max_tokens`/`max_output_tokens` is intentionally omitted here.
    // The shared `build_openai_chat_body` already drops it for DeepSeek, so
    // the official endpoint uses its 384K ceiling instead of a truncated one.
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
    let body = openai::build_chat_body(
        request,
        false,
        Some(&deepseek_extras(request, false, base_url)),
    );

    let response = http
        .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
        .bearer_auth(api_key.clone())
        // Non-streaming requests have no natural end; cap them outright.
        .timeout(std::time::Duration::from_secs(300))
        .json(&body)
        .send()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        // Issue-4 fallback for generate: gateways that reject DeepSeek-only
        // fields retry once with a bare OpenAI body.
        if status.as_u16() == 400 && mentions_deepseek_only_field(&text) {
            let plain = openai::build_chat_body(request, false, None);
            let retry = http
                .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
                .bearer_auth(api_key.clone())
                .timeout(std::time::Duration::from_secs(300))
                .json(&plain)
                .send()
                .await
                .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;
            if !retry.status().is_success() {
                let t = retry.text().await.unwrap_or_default();
                return Err(DeepSeekCodeError::Llm(t));
            }
            let value: Value = retry
                .json()
                .await
                .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;
            return openai::parse_message_response(&value);
        }
        return Err(DeepSeekCodeError::Llm(text));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    openai::parse_message_response(&value)
}

/// True when a 400 body complains about DeepSeek-only fields.
fn mentions_deepseek_only_field(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("stream_options")
        || lower.contains("thinking")
        || lower.contains("unknown field")
        || lower.contains("unrecognized")
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
    // Issue-4 fallback: if a gateway rejects `stream_options` (or `thinking`)
    // with 400, retry once without extras so custom proxies keep working.
    // The retry intentionally drops prefix-cache observability, not correctness.
    let body = openai::build_chat_body(
        request,
        true,
        Some(&deepseek_extras(request, true, base_url)),
    );

    let response = http
        .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
        .bearer_auth(api_key.clone())
        .json(&body)
        .send()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.as_u16() == 400 && mentions_deepseek_only_field(&text) {
            let plain = openai::build_chat_body(request, true, None);
            let retry = http
                .post(openai::chat_endpoint(Some(resolve_base_url(base_url))))
                .bearer_auth(api_key)
                .json(&plain)
                .send()
                .await
                .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;
            if !retry.status().is_success() {
                let t = retry.text().await.unwrap_or_default();
                return Err(DeepSeekCodeError::Llm(t));
            }
            let byte_stream = retry.bytes_stream();
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
            return Ok(Box::pin(mapped));
        }
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
