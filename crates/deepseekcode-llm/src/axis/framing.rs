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
        let Some(data) = sse_data_payload(line) else {
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
        // DeepSeek (with `stream_options.include_usage`) attaches `usage`
        // to the final chunk. Surface it so callers can bill cache hit/miss
        // instead of estimating.
        if value.get("usage").is_some() {
            if let Some(u) = parse_usage(&value) {
                events.push(Ok(LlmEvent::Usage(u)));
            }
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

pub(crate) fn sse_data_payload(line: &str) -> Option<&str> {
    let payload = line.strip_prefix("data:")?;
    let payload = payload.strip_prefix(' ').unwrap_or(payload);
    Some(payload)
}

pub(crate) fn parse_message_response(value: &Value) -> Result<crate::request::LlmResponse> {
    let choice = &value["choices"][0]["message"];
    let content = choice["content"].as_str().unwrap_or("").to_string();
    // Issue-1: preserve non-streaming reasoning so next-turn replay stays
    // byte-identical. DeepSeek returns `reasoning_content` alongside `content`.
    let reasoning_content = choice
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
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
        reasoning_content,
    })
}

fn parse_usage(value: &Value) -> Option<crate::request::TokenUsage> {
    let usage = value.get("usage")?;
    // DeepSeek reports `prompt_cache_hit_tokens / prompt_cache_miss_tokens`
    // (plus OpenAI-style `prompt_tokens_details.cached_tokens`). Normalize
    // both shapes into hit/miss so hit-rate math is provider-agnostic.
    let hit = usage
        .get("prompt_cache_hit_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
        })
        .or_else(|| {
            usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
        })
        .map(|v| v as u32);
    let miss = usage
        .get("prompt_cache_miss_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let write = usage
        .get("prompt_cache_write_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
        })
        .map(|v| v as u32);
    let input = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    // When the provider only sends hit (common on streams), derive miss from
    // `input - hit` so receipts and guards stay meaningful.
    let miss = miss.or_else(|| match (input, hit) {
        (Some(i), Some(h)) => Some(i.saturating_sub(h)),
        _ => None,
    });
    Some(crate::request::TokenUsage {
        input_tokens: input,
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
        cache_tokens: hit,
        cache_hit: hit,
        cache_miss: miss,
        cache_write: write,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_deepseek_hit_miss_shape() {
        let v = json!({
            "choices": [{"message": {"content": "hi"}}],
            "usage": {
                "prompt_tokens": 12000,
                "completion_tokens": 100,
                "prompt_cache_hit_tokens": 10800,
                "prompt_cache_miss_tokens": 1200
            }
        });
        let u = parse_usage(&v).expect("usage");
        assert_eq!(u.hit_tokens(), 10800);
        assert_eq!(u.miss_tokens(), 1200);
        assert!((u.hit_rate() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn derives_miss_from_input_minus_hit_on_streams() {
        let v = json!({
            "choices": [{"message": {"content": "hi"}}],
            "usage": {
                "prompt_tokens": 1000,
                "prompt_tokens_details": {"cached_tokens": 900}
            }
        });
        let u = parse_usage(&v).expect("usage");
        assert_eq!(u.hit_tokens(), 900);
        assert_eq!(u.miss_tokens(), 100);
    }

    #[test]
    fn sse_frame_surfaces_usage_event() {
        let frame = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"prompt_cache_hit_tokens\":900,\"prompt_cache_miss_tokens\":100}}\n\n";
        let mut acc = HashMap::new();
        let events = parse_openai_sse_frame(frame, &mut acc);
        assert!(events
            .iter()
            .any(|e| matches!(e, Ok(LlmEvent::Usage(u)) if u.hit_tokens()==900)));
    }

    #[test]
    fn non_streaming_preserves_reasoning_for_replay() {
        // Issue-1: generate path must not drop reasoning or the next turn
        // cannot replay verbatim and the prefix goes cold.
        let v = json!({
            "choices": [{"message": {"content": "hi", "reasoning_content": "think-step"}}],
            "usage": {"prompt_tokens": 10}
        });
        let r = parse_message_response(&v).expect("response");
        assert_eq!(r.reasoning_content.as_deref(), Some("think-step"));
    }
}
