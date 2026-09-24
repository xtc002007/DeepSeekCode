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
    let route = OpenAiRoute::new();
    let api_key = api_key
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| DeepSeekCodeError::Llm("OpenAI API key not set".into()))?;
    let body = route.protocol.build_body(request, false);
    let url = route.endpoint.resolve(base_url);

    let response = crate::http_client::post_json_with_fallback(http, &url, &api_key, &body, true)
        .await
        .map_err(|combined| DeepSeekCodeError::Llm(format!("{url}: {combined}")))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(DeepSeekCodeError::Llm(text));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    crate::axis::framing::parse_message_response(&value)
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let route = OpenAiRoute::new();
    let api_key = api_key
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| DeepSeekCodeError::Llm("OpenAI API key not set".into()))?;
    let body = route.protocol.build_body(request, true);
    let url = route.endpoint.resolve(base_url);
    let framing = route.framing;

    let response = crate::http_client::post_json_with_fallback(http, &url, &api_key, &body, false)
        .await
        .map_err(|combined| DeepSeekCodeError::Llm(format!("{url}: {combined}")))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(DeepSeekCodeError::Llm(text));
    }

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
