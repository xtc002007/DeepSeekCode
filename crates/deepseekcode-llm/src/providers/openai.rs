use crate::axis::protocol::{merge_extra, Protocol};
use crate::axis::{
    auth::Auth, endpoint::Endpoint, framing::Framing, BearerAuth, OpenAiChatEndpoint,
    OpenAiChatProtocol, SseFraming,
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
    auth: BearerAuth,
    protocol: OpenAiChatProtocol,
    framing: SseFraming,
}

impl OpenAiRoute {
    fn new() -> Self {
        Self {
            endpoint: OpenAiChatEndpoint,
            auth: BearerAuth,
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
    let api_key = api_key.ok_or_else(|| DeepSeekCodeError::Llm("OpenAI API key not set".into()))?;
    let body = route.protocol.build_body(request, false);
    let url = route.endpoint.resolve(base_url);

    let response = route
        .auth
        .apply(http.post(url), &api_key)
        // Non-streaming requests have no natural end; cap them outright.
        .timeout(std::time::Duration::from_secs(300))
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

    crate::axis::framing::parse_message_response(&value)
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let route = OpenAiRoute::new();
    let api_key = api_key.ok_or_else(|| DeepSeekCodeError::Llm("OpenAI API key not set".into()))?;
    let body = route.protocol.build_body(request, true);
    let url = route.endpoint.resolve(base_url);
    let framing = route.framing;

    let response = route
        .auth
        .apply(http.post(url), &api_key)
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
            move |state, chunk| {
                let (buffer, tool_acc) = state;
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let mut events = Vec::new();
                        while let Some(pos) = buffer.find("\n\n") {
                            let frame = buffer.drain(..=pos).collect::<String>();
                            events.extend(framing.parse_sse_frame(&frame, tool_acc));
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

pub(crate) fn parse_sse_frame(
    frame: &str,
    tool_acc: &mut HashMap<usize, (String, String, String)>,
) -> Vec<Result<LlmEvent>> {
    SseFraming.parse_sse_frame(frame, tool_acc)
}

pub(crate) fn parse_message_response(value: &Value) -> Result<crate::request::LlmResponse> {
    crate::axis::framing::parse_message_response(value)
}
