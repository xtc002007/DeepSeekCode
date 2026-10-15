use crate::event::LlmEvent;
use crate::tool_call::LlmToolCall;
use code_plus_plus_types::{CodePlusPlusError, Result};
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

/// Shared SSE splitter: single source of truth for all streaming paths
/// (OpenAI / DeepSeek reqwest + pinned hyper + Anthropic).
///
/// - `Vec<u8>` buffering: never splits multi-byte UTF-8 across chunks
///   (trailing incomplete sequences stay buffered, no mojibake);
/// - CRLF normalized (`\r\n` -> `\n`) on the decodable prefix so
///   `\r\n\r\n` frames always split;
/// - 1 MiB cap with warn + clear (unbounded growth previously OOM'd);
/// - only complete `\n\n`-terminated frames with successful UTF-8 decode
///   are consumed; remainder stays buffered.
pub struct SseSplitter {
    buf: Vec<u8>,
}

impl SseSplitter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append `chunk`, return all complete frames (each including its
    /// trailing `\n\n`). Incomplete tail stays buffered.
    pub fn push_and_split(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > 1024 * 1024 {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                buffer_len = self.buf.len(),
                "llm sse buffer overflow; truncating"
            );
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                "llm sse truncation triggered"
            );
            self.buf.clear();
            return Vec::new();
        }
        // Exclude trailing incomplete UTF-8 sequence from decoding so a
        // multi-byte char split across chunks is never lossy-replaced.
        let incomplete = trailing_incomplete_utf8_len(&self.buf);
        let decodable_len = self.buf.len() - incomplete;
        let text = String::from_utf8_lossy(&self.buf[..decodable_len]).to_string();
        let normalized = text.replace("\r\n", "\n");
        let mut frames = Vec::new();
        let mut remainder = normalized.as_str();
        while let Some(pos) = remainder.find("\n\n") {
            frames.push(remainder[..pos + 2].to_string());
            remainder = &remainder[pos + 2..];
        }
        // `remainder` is the normalized unconsumed tail. Re-encode it plus
        // the undecodable trailing bytes; buffer stays normalized so raw vs
        // normalized length mismatch can never cause over-drain.
        // `floor_char_boundary` guarantees the drain lands on UTF-8 boundary.
        let mut next = remainder.as_bytes().to_vec();
        next.extend_from_slice(&self.buf[decodable_len..]);
        self.buf = next;
        // `floor_char_boundary` note: `remainder` derives from a valid
        // `String`, so `remainder.as_bytes()` is always boundary-safe; no
        // manual `while !is_char_boundary` drain loop is needed.
        frames
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

impl Default for SseSplitter {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes at the end of `buf` forming an incomplete UTF-8 sequence (0-3).
fn trailing_incomplete_utf8_len(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    // Walk back over continuation bytes (10xxxxxx), max 3.
    let mut cont = 0usize;
    let mut i = buf.len();
    while i > 0 && cont < 3 {
        let b = buf[i - 1];
        if (0x80..=0xBF).contains(&b) {
            cont += 1;
            i -= 1;
        } else {
            break;
        }
    }
    if i == 0 {
        // All continuation bytes with no lead: keep all buffered.
        return buf.len();
    }
    let lead = buf[i - 1];
    if lead < 0x80 {
        return 0;
    }
    let expected: usize = if lead >= 0xF0 {
        4
    } else if lead >= 0xE0 {
        3
    } else if lead >= 0xC0 {
        2
    } else {
        // Stray continuation start: treat trailing `cont` as incomplete.
        return cont;
    };
    let have = cont + 1;
    if have < expected {
        have
    } else {
        0
    }
}

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
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        frame_summary = %code_plus_plus_telemetry::summarize_prompt(frame, false),
        "llm sse frame parse start"
    );
    let mut events = Vec::new();
    for line in frame.lines() {
        let Some(data) = sse_data_payload(line) else {
            continue;
        };
        if data == "[DONE]" {
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                "llm sse [DONE] received"
            );
            // De-duplicate: only emit Completed if not already emitted for
            // this frame batch (finish_reason also emits one below).
            if !events.iter().any(|e| matches!(e, Ok(LlmEvent::Completed))) {
                events.push(Ok(LlmEvent::Completed));
            }
            continue;
        }
        let value: Value = match serde_json::from_str::<Value>(data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    error = %e,
                    data_summary = %code_plus_plus_telemetry::summarize_prompt(data, false),
                    "llm sse frame JSON parse failed; skipping line"
                );
                tracing::error!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    error = %e,
                    "llm protocol error: sse JSON unparseable"
                );
                continue;
            }
        };
        // Error shapes: `{error:{message}}`, `{error:{code,type}}`,
        // `{error:"string"}`, `{message:"..."}` (P0-5: previously only
        // `error.message` was recognized).
        if value.get("error").is_some() || value.get("message").is_some() {
            let msg = value["error"]["message"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .or_else(|| value["message"].as_str())
                .map(|s| {
                    let code = value["error"]["code"]
                        .as_str()
                        .or_else(|| value["error"]["type"].as_str())
                        .unwrap_or("");
                    if code.is_empty() {
                        s.to_string()
                    } else {
                        format!("{s} ({code})")
                    }
                })
                .unwrap_or_else(|| value.to_string());
            // Sanitize: cap length, strip key-like fragments.
            let sanitized = sanitize_llm_error(&msg);
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                error_summary = %code_plus_plus_telemetry::summarize_prompt(&sanitized, false),
                "llm sse frame carried provider error"
            );
            if crate::overflow::is_context_overflow(&sanitized) {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    error_summary = %code_plus_plus_telemetry::summarize_prompt(&sanitized, false),
                    "llm sse error indicates context overflow"
                );
            }
            if msg.len() > 500 {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    raw_len = msg.len(),
                    "llm sse error text truncated for log"
                );
            }
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                "llm sse provider error surfaced"
            );
            events.push(Err(CodePlusPlusError::Llm(sanitized)));
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
        if entry_too_large(&*tool_acc) {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                acc_entries = tool_acc.len(),
                "llm sse tool accumulator overlarge; truncation risk"
            );
        }
        // `tool_calls` finish: no duplicate empty-argument deltas (the
        // incremental deltas already carried arguments). Only ensure a
        // single Completed per turn. On completion, validate concatenated
        // tool arguments: failures surface as `LlmEvent::Error` (via
        // `Err`) instead of being persisted as silent `{}` downstream.
        if matches!(
            choice["finish_reason"].as_str(),
            Some("stop") | Some("tool_calls")
        ) && !events.iter().any(|e| matches!(e, Ok(LlmEvent::Completed)))
        {
            for (index, (_, name, args)) in tool_acc.iter() {
                if args.is_empty() {
                    continue;
                }
                if let Err(e) = LlmToolCall::parse_arguments_strict(args) {
                    let sanitized = sanitize_llm_error(args);
                    tracing::error!(
                        target: code_plus_plus_telemetry::LLM,
                        run_id = %code_plus_plus_telemetry::current_run_id(),
                        tool_index = index,
                        tool = %name,
                        error = %e,
                        raw_summary = %code_plus_plus_telemetry::summarize_prompt(&sanitized, false),
                        "llm sse concatenated tool arguments invalid; surfacing Error"
                    );
                    events.push(Err(CodePlusPlusError::Llm(format!(
                        "invalid tool arguments JSON for '{name}': {e}"
                    ))));
                }
            }
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                finish_reason = %choice["finish_reason"].as_str().unwrap_or(""),
                event_count = events.len(),
                "llm sse turn completed"
            );
            events.push(Ok(LlmEvent::Completed));
        }
    }
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        event_count = events.len(),
        "llm sse frame parse completed"
    );
    events
}

fn entry_too_large(acc: &HashMap<usize, (String, String, String)>) -> bool {
    acc.values().any(|(_, _, args)| args.len() > 512 * 1024)
}

pub(crate) fn sse_data_payload(line: &str) -> Option<&str> {
    let payload = line.strip_prefix("data:")?;
    let payload = payload.strip_prefix(' ').unwrap_or(payload);
    Some(payload)
}

pub(crate) fn parse_message_response(value: &Value) -> Result<crate::request::LlmResponse> {
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        body_keys = ?value.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
        "llm parse_message_response start"
    );
    // Malformed provider payloads previously defaulted silently (`""`, `{}`,
    // `[]`), masking gateway bugs as empty turns. Warn with shape diagnostics;
    // behavior (defaults) is unchanged for back-compat.
    if value.get("choices").and_then(|c| c.as_array()).is_none() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            body_keys = ?value.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
            "llm response has no choices array; defaulting to empty content"
        );
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm protocol error: missing choices array"
        );
    }
    let choice = &value["choices"][0]["message"];
    if choice.is_null() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm response choices[0].message is missing; defaulting to empty content"
        );
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm protocol error: missing message object"
        );
    }
    let content = choice["content"].as_str().unwrap_or("").to_string();
    if choice.get("content").is_some() && choice["content"].as_str().is_none() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm response content not a string; truncated to empty"
        );
    }
    if content.len() > 1_000_000 {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            content_len = content.len(),
            content_summary = %code_plus_plus_telemetry::summarize_prompt(&content, false),
            "llm response content overlarge; truncation risk"
        );
    }
    // Issue-1: preserve non-streaming reasoning so next-turn replay stays
    // byte-identical. DeepSeek returns `reasoning_content` alongside `content`.
    let reasoning_content = choice
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let tool_calls: Vec<LlmToolCall> = choice["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .map(|c| {
                    let raw_args = c["function"]["arguments"].as_str().unwrap_or("{}");
                    let arguments = match LlmToolCall::parse_arguments_strict(raw_args) {
                        Ok(args) => args,
                        Err(e) => {
                            tracing::warn!(
                                target: code_plus_plus_telemetry::LLM,
                                run_id = %code_plus_plus_telemetry::current_run_id(),
                                tool = %c["function"]["name"].as_str().unwrap_or(""),
                                error = %e,
                                raw_summary = %code_plus_plus_telemetry::summarize_prompt(raw_args, false),
                                "llm tool arguments unparseable; surfacing _parse_error"
                            );
                            tracing::error!(
                                target: code_plus_plus_telemetry::LLM,
                                run_id = %code_plus_plus_telemetry::current_run_id(),
                                tool = %c["function"]["name"].as_str().unwrap_or(""),
                                error = %e,
                                "llm tool_call parse error in non-stream response"
                            );
                            // Non-streaming keeps the error surfaced instead of
                            // silently substituting `{}` (which masked
                            // provider bugs as empty turns).
                            serde_json::json!({ "_parse_error": raw_args })
                        }
                    };
                    LlmToolCall {
                        id: c["id"].as_str().unwrap_or("").to_string(),
                        name: c["function"]["name"].as_str().unwrap_or("").to_string(),
                        arguments,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let usage = parse_usage(value);
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        content_len = content.len(),
        tool_call_count = tool_calls.len(),
        output_tokens = ?usage.as_ref().and_then(|u| u.output_tokens),
        "llm parse_message_response succeeded"
    );
    Ok(crate::request::LlmResponse {
        content,
        tool_calls,
        usage,
        reasoning_content,
    })
}

/// Strip key-like fragments and cap length so provider error text can go
/// to SSE/logs without leaking endpoint/key material (P0-5).
pub(crate) fn sanitize_llm_error(raw: &str) -> String {
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        raw_len = raw.len(),
        "llm sanitize error start"
    );
    let mut s = raw.trim().to_string();
    let had_secret = ["sk-", "Bearer ", "x-api-key"]
        .iter()
        .any(|p| s.contains(p));
    // Mask `sk-...` / `Bearer ...` fragments.
    for prefix in ["sk-", "Bearer ", "x-api-key"] {
        while let Some(pos) = s.find(prefix) {
            let end = s[pos..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map(|i| pos + i)
                .unwrap_or(s.len());
            s.replace_range(pos..end, "[REDACTED]");
            if s.len() > 2000 {
                break;
            }
        }
    }
    if had_secret {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm error contained key-like fragment; redacted"
        );
    }
    if s.len() > 500 {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            raw_len = s.len(),
            "llm error overlong; truncating to 500 for log"
        );
        s.truncate(500);
    }
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        sanitized_len = s.len(),
        "llm error sanitized"
    );
    if s.trim().is_empty() && !raw.trim().is_empty() {
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm sanitize produced empty error"
        );
    }
    s
}

fn parse_usage(value: &Value) -> Option<crate::request::TokenUsage> {
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        has_usage = value.get("usage").is_some(),
        "llm parse_usage start"
    );
    let usage = value.get("usage")?;
    // Saturating u64->u32 audit: detect truncation that would blind billing.
    for key in [
        "prompt_tokens",
        "completion_tokens",
        "input_tokens",
        "output_tokens",
    ] {
        if let Some(v) = usage.get(key).and_then(|x| x.as_u64()) {
            if u32::try_from(v).is_err() {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    key,
                    raw_value = v,
                    "llm usage token count truncated to u32::MAX"
                );
            }
        }
    }
    // Saturating u64->u32 (P0-5: previously silent `as u32` truncation).
    let sat = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
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
        .map(sat);
    let miss = usage
        .get("prompt_cache_miss_tokens")
        .and_then(|v| v.as_u64())
        .map(sat);
    let write = usage
        .get("prompt_cache_write_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
        })
        .map(sat);
    let input = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .map(sat);
    // When the provider only sends hit (common on streams), derive miss from
    // `input - hit` so receipts and guards stay meaningful.
    let miss = miss.or_else(|| match (input, hit) {
        (Some(i), Some(h)) => Some(i.saturating_sub(h)),
        _ => None,
    });
    let out = crate::request::TokenUsage {
        input_tokens: input,
        output_tokens: usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .map(sat),
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .or_else(|| usage.get("reasoning_tokens"))
            .and_then(|v| v.as_u64())
            .map(sat),
        cache_tokens: hit,
        cache_hit: hit,
        cache_miss: miss,
        cache_write: write,
    };
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        input_tokens = ?out.input_tokens,
        output_tokens = ?out.output_tokens,
        cache_hit = out.hit_tokens(),
        cache_miss = out.miss_tokens(),
        "llm usage parsed"
    );
    if out.input_tokens.is_none() && out.output_tokens.is_none() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm usage empty; billing blind"
        );
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            "llm usage parse produced no token counts"
        );
    }
    Some(out)
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
