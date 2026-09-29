use crate::axis::protocol::{merge_extra, Protocol};
use crate::axis::{
    endpoint::Endpoint, framing::Framing, OpenAiChatEndpoint, OpenAiChatProtocol, SseFraming,
};
use crate::event::LlmEvent;
use crate::request::LlmRequest;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;

struct OpenAiRoute {
    endpoint: OpenAiChatEndpoint,
    protocol: OpenAiChatProtocol,
    framing: SseFraming,
}

impl OpenAiRoute {
    fn new() -> Self {
        Self {
            endpoint: OpenAiChatEndpoint,
            protocol: OpenAiChatProtocol,
            framing: SseFraming,
        }
    }
}

pub(crate) fn chat_endpoint(base_url: Option<&str>) -> String {
    OpenAiChatEndpoint.resolve(base_url)
}

pub(crate) fn build_chat_body(request: &LlmRequest, stream: bool, extra: Option<&Value>) -> Value {
    let route = OpenAiRoute::new();
    let mut body = route.protocol.build_body(request, stream);
    merge_extra(&mut body, extra);
    body
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<crate::request::LlmResponse> {
    let start = std::time::Instant::now();
    let route = OpenAiRoute::new();
    let url = route.endpoint.resolve(base_url);
    let endpoint_host = crate::redact_endpoint_for_log(&url);
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "openai",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
        "llm openai generate start"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "openai",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm openai generate debug"
    );
    crate::providers::log_provider_route("openai", &request.model, &endpoint_host);
    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                error = "OpenAI API key not set",
                "llm auth failed"
            );
            return Err(DeepSeekCodeError::Llm("OpenAI API key not set".into()));
        }
    };
    let body = route.protocol.build_body(request, false);

    let response = match crate::http_client::post_json_with_fallback(
        http, &url, &api_key, &body, true,
    )
    .await
    {
        Ok(r) => r,
        Err(combined) => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error_summary = %deepseekcode_telemetry::summarize_prompt(&combined, false),
                "llm openai generate transport failed"
            );
            crate::providers::log_provider_error("openai", &request.model, &combined);
            return Err(DeepSeekCodeError::Llm(format!("{url}: {combined}")));
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let sanitized = crate::axis::framing::sanitize_llm_error(&text);
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "openai",
            model = %request.model,
            endpoint_host = %endpoint_host,
            status = %status,
            duration_ms = start.elapsed().as_millis() as u64,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm openai generate non-success status"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "openai",
            model = %request.model,
            status = %status,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm openai generate failed"
        );
        if crate::overflow::is_context_overflow(&sanitized) {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                "llm openai generate context overflow"
            );
        }
        return Err(DeepSeekCodeError::Llm(text));
    }

    let status = response.status();
    let value: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "llm openai generate response JSON parse failed"
            );
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                error = %e,
                "llm openai generate parse error"
            );
            return Err(DeepSeekCodeError::Llm(e.to_string()));
        }
    };

    match crate::axis::framing::parse_message_response(&value) {
        Ok(resp) => {
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                status = %status,
                output_tokens = ?resp.usage.as_ref().and_then(|u| u.output_tokens),
                tool_call_count = resp.tool_calls.len(),
                "llm openai generate succeeded"
            );
            Ok(resp)
        }
        Err(e) => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "llm openai generate response parse failed"
            );
            Err(e)
        }
    }
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let start = std::time::Instant::now();
    let route = OpenAiRoute::new();
    let url = route.endpoint.resolve(base_url);
    let endpoint_host = crate::redact_endpoint_for_log(&url);
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "openai",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
        "llm openai stream start"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "openai",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm openai stream debug"
    );
    crate::providers::log_provider_route("openai", &request.model, &endpoint_host);
    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                endpoint_host = %endpoint_host,
                error = "OpenAI API key not set",
                "llm auth failed"
            );
            return Err(DeepSeekCodeError::Llm("OpenAI API key not set".into()));
        }
    };
    let body = route.protocol.build_body(request, true);
    let framing = route.framing;

    let response =
        match crate::http_client::post_json_with_fallback(http, &url, &api_key, &body, false).await
        {
            Ok(r) => r,
            Err(combined) => {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = "openai",
                    model = %request.model,
                    endpoint_host = %endpoint_host,
                    duration_ms = start.elapsed().as_millis() as u64,
                    error_summary = %deepseekcode_telemetry::summarize_prompt(&combined, false),
                    "llm openai stream transport failed"
                );
                crate::providers::log_provider_error("openai", &request.model, &combined);
                return Err(DeepSeekCodeError::Llm(format!("{url}: {combined}")));
            }
        };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let sanitized = crate::axis::framing::sanitize_llm_error(&text);
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "openai",
            model = %request.model,
            endpoint_host = %endpoint_host,
            status = %status,
            duration_ms = start.elapsed().as_millis() as u64,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm openai stream non-success status"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = "openai",
            model = %request.model,
            status = %status,
            error_summary = %deepseekcode_telemetry::summarize_prompt(&sanitized, false),
            "llm openai stream failed"
        );
        if crate::overflow::is_context_overflow(&sanitized) {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = "openai",
                model = %request.model,
                "llm openai stream context overflow"
            );
        }
        return Err(DeepSeekCodeError::Llm(text));
    }
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = "openai",
        model = %request.model,
        endpoint_host = %endpoint_host,
        duration_ms = start.elapsed().as_millis() as u64,
        status = %response.status(),
        "llm openai stream opened"
    );

    let byte_stream = response.bytes_stream();
    let mapped = byte_stream
        .map(|chunk| chunk.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
        .scan(
            (
                Vec::<u8>::new(),
                HashMap::<usize, (String, String, String)>::new(),
            ),
            move |state, chunk| {
                let (buffer, tool_acc) = state;
                match chunk {
                    Ok(bytes) => {
                        // Byte-level buffering: never split multi-byte UTF-8
                        // across chunks (P0-5 mojibake fix).
                        buffer.extend_from_slice(&bytes);
                        if buffer.len() > 1024 * 1024 {
                            tracing::warn!(
                                target: deepseekcode_telemetry::LLM,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                provider = "openai",
                                buffer_len = buffer.len(),
                                "llm openai stream buffer overflow; truncating"
                            );
                            tracing::error!(
                                target: deepseekcode_telemetry::LLM,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                provider = "openai",
                                "llm openai stream truncation triggered"
                            );
                            buffer.clear();
                            return futures::future::ready(Some(vec![]));
                        }
                        let text = String::from_utf8_lossy(buffer).to_string();
                        let normalized = text.replace("\r\n", "\n");
                        let mut events = Vec::new();
                        // Split off all complete `\n\n`-terminated frames.
                        let mut remainder = normalized.as_str();
                        let mut consumed_chars = 0usize;
                        while let Some(pos) = remainder.find("\n\n") {
                            let frame = &remainder[..pos + 2];
                            events.extend(framing.parse_sse_frame(frame, tool_acc));
                            consumed_chars += frame.len();
                            remainder = &remainder[pos + 2..];
                        }
                        // Drain consumed bytes (char-boundary safe via String).
                        let consumed_bytes = text.len() - remainder.len();
                        let drain = consumed_bytes.min(buffer.len());
                        // Ensure drain lands on a UTF-8 boundary.
                        let mut drain = drain;
                        while drain > 0 && !text.is_char_boundary(text.len() - remainder.len()) {
                            drain = drain.saturating_sub(1);
                        }
                        let _ = buffer.drain(..drain);
                        let _ = consumed_chars;
                        futures::future::ready(Some(events))
                    }
                    Err(e) => futures::future::ready(Some(vec![Err(e)])),
                }
            },
        )
        .flat_map(futures::stream::iter);

    Ok(Box::pin(mapped))
}

pub(crate) fn parse_sse_frame(
    frame: &str,
    tool_acc: &mut HashMap<usize, (String, String, String)>,
) -> Vec<Result<LlmEvent>> {
    SseFraming.parse_sse_frame(frame, tool_acc)
}

pub(crate) fn parse_message_response(value: &Value) -> Result<crate::request::LlmResponse> {
    crate::axis::framing::parse_message_response(value)
}
