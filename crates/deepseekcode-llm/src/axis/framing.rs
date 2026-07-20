use crate::event::LlmEvent;
use crate::tool_call::LlmToolCall;
use deepseekcode_types::{DeepSeekCodeError, Result};
use serde_json::Value;
use std::collections::HashMap;

pub trait Framing {
    fn parse_sse_frame(
        &self,
        frame: &str,
        tool_acc: &mut HashMap<usize, (String, String, String)>,
    ) -> Vec<Result<LlmEvent>>;
}

pub struct SseFraming;

impl Framing for SseFraming {
    fn parse_sse_frame(
        &self,
        frame: &str,
        tool_acc: &mut HashMap<usize, (String, String, String)>,
    ) -> Vec<Result<LlmEvent>> {
        parse_openai_sse_frame(frame, tool_acc)
    }
}

pub fn parse_openai_sse_frame(
    frame: &str,
    tool_acc: &mut HashMap<usize, (String, String, String)>,
) -> Vec<Result<LlmEvent>> {
    let mut events = Vec::new();
    for line in frame.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data == "[DONE]" {
            events.push(Ok(LlmEvent::Completed));
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(err) = value["error"]["message"].as_str() {
            events.push(Err(DeepSeekCodeError::Llm(err.to_string())));
            continue;
        }
        let choice = &value["choices"][0];
        let delta = &choice["delta"];
        if let Some(reasoning) = delta["reasoning_content"].as_str() {
            if !reasoning.is_empty() {
                events.push(Ok(LlmEvent::ReasoningDelta(reasoning.to_string())));
            }
        }
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                events.push(Ok(LlmEvent::TextDelta(text.to_string())));
            }
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                let entry = tool_acc.entry(index).or_insert_with(|| {
                    (
                        call["id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .unwrap_or_else(|| format!("call_{index}")),
                        call["function"]["name"].as_str().unwrap_or("").to_string(),
                        String::new(),
                    )
                });
                if let Some(id) = call["id"].as_str() {
                    if !id.is_empty() {
                        entry.0 = id.to_string();
                    }
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    if !name.is_empty() {
                        entry.1 = name.to_string();
                    }
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    entry.2.push_str(args);
                    events.push(Ok(LlmEvent::ToolCallDelta {
                        index,
                        id: entry.0.clone(),
                        name: entry.1.clone(),
                        arguments: args.to_string(),
                    }));
                }
            }
        }
        if choice["finish_reason"].as_str() == Some("tool_calls") {
            for (index, (id, name, _args)) in tool_acc.iter() {
                events.push(Ok(LlmEvent::ToolCallDelta {
                    index: *index,
                    id: id.clone(),
                    name: name.clone(),
                    arguments: String::new(),
                }));
            }
        }
        if choice["finish_reason"].as_str() == Some("stop")
            || choice["finish_reason"].as_str() == Some("tool_calls")
        {
            events.push(Ok(LlmEvent::Completed));
        }
    }
    events
}

pub(crate) fn parse_message_response(value: &Value) -> Result<crate::request::LlmResponse> {
    let choice = &value["choices"][0]["message"];
    let content = choice["content"].as_str().unwrap_or("").to_string();
    let tool_calls = choice["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .map(|c| LlmToolCall {
                    id: c["id"].as_str().unwrap_or("").to_string(),
                    name: c["function"]["name"].as_str().unwrap_or("").to_string(),
                    arguments: serde_json::from_str(
                        c["function"]["arguments"].as_str().unwrap_or("{}"),
                    )
                    .unwrap_or(serde_json::json!({})),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(crate::request::LlmResponse {
        content,
        tool_calls,
        usage: parse_usage(value),
    })
}

fn parse_usage(value: &Value) -> Option<crate::request::TokenUsage> {
    let usage = value.get("usage")?;
    Some(crate::request::TokenUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .or_else(|| usage.get("reasoning_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .or_else(|| usage.get("cache_read_input_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}
