//! LLM message repair helpers and token estimation for session runner.

use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_llm::{LlmMessage, LlmRole, LlmToolCall};
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::message::{Message, PartKind};
use serde_json::json;

pub(crate) fn provider_tool_call_id(call: &LlmToolCall) -> String {
    if call.id.is_empty() {
        ulid::Ulid::new().to_string()
    } else {
        call.id.clone()
    }
}

pub(crate) fn message_text(msg: &Message) -> String {
    msg.parts
        .iter()
        .filter_map(|part| match part.kind {
            PartKind::Text => part.text.clone(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn message_reasoning(msg: &Message) -> Option<String> {
    let reasoning: Vec<String> = msg
        .parts
        .iter()
        .filter_map(|part| match part.kind {
            PartKind::Reasoning => part.text.clone(),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect();
    if reasoning.is_empty() {
        None
    } else {
        Some(reasoning.join("\n"))
    }
}

/// DeepSeek prefix-cache guard: keep the provider-visible tool projection
/// bounded (32KiB) and byte-stable. Full output stays in storage/managed
/// files; this only bounds what enters the request prefix so one huge `cat`
/// cannot permanently poison the cache. Old prefixes stay byte-identical.
pub(crate) const MAX_TOOL_PREFIX_BYTES: usize = 32 * 1024;
/// Tail cap for head/tail truncation: error tails stay short so JSON and
/// plain-text projections split identically (previously 8KiB vs unbounded
/// per branch, making prefix bytes jitter on content type).
pub(crate) const TRUNC_TAIL_CAP_BYTES: usize = 8 * 1024;

/// Shared head/tail split: 2/3 head, remainder tail (capped), minus marker room.
fn head_tail_budgets() -> (usize, usize) {
    let head_budget = MAX_TOOL_PREFIX_BYTES * 2 / 3;
    let tail_budget = (MAX_TOOL_PREFIX_BYTES - head_budget - 1024).min(TRUNC_TAIL_CAP_BYTES);
    (head_budget, tail_budget)
}

pub(crate) fn truncate_tool_content_for_prefix(content: &str) -> String {
    if content.len() <= MAX_TOOL_PREFIX_BYTES {
        return content.to_string();
    }
    // Issue-10: preserve recovery metadata. Tool output is usually an envelope
    // JSON containing `full_output_file`/`truncated`/`preview`. Blind head/tail
    // could slice the file path itself away, leaving the model aware of
    // truncation but unable to recover. Truncate long string leaves instead.
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(content) {
        if truncate_json_strings(&mut value, MAX_TOOL_PREFIX_BYTES) {
            if let Ok(bounded) = serde_json::to_string(&value) {
                if bounded.len() <= MAX_TOOL_PREFIX_BYTES {
                    return bounded;
                }
            }
        }
        // Fall through to head/tail, but re-attach the recovery path.
        let recovery = extract_recovery_hint(&value);
        let (head_budget, tail_budget) = head_tail_budgets();
        let head = snap_to_char_boundary(content, head_budget);
        let tail = snap_to_char_boundary_tail(content, tail_budget);
        let mut out = format!(
            "{head}\n…[truncated {} bytes — full output retained locally; recover with tool_result_page]…\n{tail}",
            content.len().saturating_sub(head.len() + tail.len())
        );
        if !recovery.is_empty() {
            out.push_str(&format!("\n{recovery}"));
        }
        return snap_to_char_boundary(&out, MAX_TOOL_PREFIX_BYTES);
    }
    // Head/tail split preserves errors (usually at the tail) while staying
    // within budget. Snap to char boundaries to avoid splitting UTF-8.
    let (head_budget, tail_budget) = head_tail_budgets();
    let head = snap_to_char_boundary(content, head_budget);
    let tail = snap_to_char_boundary_tail(content, tail_budget);
    format!(
        "{head}\n…[truncated {} bytes — full output retained locally; recover with tool_result_page]…\n{tail}",
        content.len().saturating_sub(head.len() + tail.len())
    )
}

/// Truncate long string leaves in place, keeping `full_output_file`,
/// `truncated`, `title` and other recovery keys intact. Returns true when
/// anything was shortened.
fn truncate_json_strings(value: &mut serde_json::Value, budget: usize) -> bool {
    use serde_json::Value::*;
    match value {
        String(s) => {
            if s.len() > budget / 4 && s.len() > 1024 {
                let keep = (budget / 4).max(1024);
                let head = snap_to_char_boundary(s, keep * 2 / 3);
                let tail = snap_to_char_boundary_tail(s, keep / 3);
                *s = format!("{head}\n…[truncated — recover with tool_result_page]…\n{tail}");
                true
            } else {
                false
            }
        }
        Array(arr) => {
            let mut changed = false;
            for v in arr.iter_mut() {
                changed |= truncate_json_strings(v, budget);
            }
            changed
        }
        Object(map) => {
            let mut changed = false;
            // Never truncate recovery keys themselves.
            for (k, v) in map.iter_mut() {
                if matches!(
                    k.as_str(),
                    "full_output_file" | "truncated" | "title" | "tool" | "duration_ms"
                ) {
                    continue;
                }
                changed |= truncate_json_strings(v, budget);
            }
            changed
        }
        _ => false,
    }
}

/// Extract a one-line recovery hint that survives even the head/tail path.
fn extract_recovery_hint(value: &serde_json::Value) -> String {
    let mut hints = Vec::new();
    if let Some(path) = value
        .get("full_output_file")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("output")
                .and_then(|o| o.get("full_output_file"))
                .and_then(|v| v.as_str())
        })
    {
        hints.push(format!(
            "full_output_file: {path} (read via tool_result_page)"
        ));
    }
    hints.join("\n")
}

fn snap_to_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn snap_to_char_boundary_tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = s.len().saturating_sub(max_bytes);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// P1-5 transport-level 400 recovery predicate (pure, unit-tested).
/// Returns true only when ALL hold, bounding recovery to exactly one
/// reasoning-stripped retry per turn:
/// - the failure names a reasoning/tool shape (`reasoning`, `tool_calls`)
///   with a 400-class signal, so stripping `reasoning_content` can fix it;
/// - the request actually carried reasoning (else the retry would resend
///   identical bytes — pure waste);
/// - it is NOT a context overflow (dedicated recovery owns that path), nor
///   auth/quota/rate/network failure (retrying those is pointless or harmful).
pub(crate) fn should_retry_without_reasoning(err_text: &str, had_reasoning: bool) -> bool {
    if !had_reasoning {
        return false;
    }
    if deepseekcode_llm::is_context_overflow(err_text) {
        return false;
    }
    let lower = err_text.to_ascii_lowercase();
    let mentions_400 = lower.contains("400")
        || lower.contains("bad request")
        || lower.contains("invalid_request")
        || lower.contains("invalid request");
    let mentions_shape =
        lower.contains("reasoning") || lower.contains("tool_calls") || lower.contains("tool_call");
    if !(mentions_400 && mentions_shape) {
        return false;
    }
    const EXCLUDED: &[&str] = &[
        "unauthorized",
        "forbidden",
        "401",
        "403",
        "api key",
        "apikey",
        "quota",
        "rate",
        "permission",
        "billing",
        "credit",
        "timed out",
        "timeout",
        "connection",
        "network",
        "dns",
    ];
    !EXCLUDED.iter().any(|s| lower.contains(s))
}

pub(crate) fn repair_llm_tool_chain(messages: &mut Vec<LlmMessage>) {
    tracing::debug!(
        target: deepseekcode_telemetry::SESSION,
        run_id = %deepseekcode_telemetry::current_run_id(),
        messages = messages.len(),
        "repair_llm_tool_chain started"
    );
    if std::env::var("DEEPSEEKCODE_STRICT_TOOL_CHAIN")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
    {
        return;
    }
    repair_missing_tool_results(messages);
    repair_missing_assistant_tool_calls(messages);
}

fn repair_missing_tool_results(messages: &mut Vec<LlmMessage>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != LlmRole::Assistant || messages[i].tool_calls.is_empty() {
            i += 1;
            continue;
        }

        let calls = messages[i].tool_calls.clone();
        let mut j = i + 1;
        let mut answered = std::collections::HashSet::new();
        while j < messages.len() && messages[j].role == LlmRole::Tool {
            if let Some(id) = &messages[j].tool_call_id {
                answered.insert(id.clone());
            }
            j += 1;
        }

        let mut synthetic = Vec::new();
        for call in calls {
            if answered.contains(&call.id) {
                continue;
            }
            tracing::warn!(
                target: deepseekcode_telemetry::SESSION,
                run_id = %deepseekcode_telemetry::current_run_id(),
                tool_call_id = %call.id,
                tool = %call.name,
                assistant_index = i,
                "repairing missing tool result for assistant tool_call_id"
            );
            synthetic.push(LlmMessage {
                role: LlmRole::Tool,
                content: r#"{"error":"tool execution was interrupted or did not complete","success":false}"#
                    .into(),
                tool_call_id: Some(call.id),
                name: Some(call.name),
                tool_calls: Vec::new(),
                reasoning_content: None,
            });
        }

        if synthetic.is_empty() {
            i = j;
            continue;
        }

        let insert_at = j;
        let advance = synthetic.len();
        messages.splice(insert_at..insert_at, synthetic);
        i = insert_at + advance;
    }
}

fn repair_missing_assistant_tool_calls(messages: &mut [LlmMessage]) {
    for i in 0..messages.len() {
        if messages[i].role != LlmRole::Tool {
            continue;
        }
        let Some(tool_id) = messages[i].tool_call_id.clone() else {
            continue;
        };
        let mut assistant_idx = None;
        for j in (0..i).rev() {
            match messages[j].role {
                LlmRole::Assistant => {
                    assistant_idx = Some(j);
                    break;
                }
                LlmRole::Tool => continue,
                _ => break,
            }
        }
        let Some(a_idx) = assistant_idx else {
            continue;
        };
        let has_call = messages[a_idx]
            .tool_calls
            .iter()
            .any(|call| call.id == tool_id);
        if has_call {
            continue;
        }
        let name = messages[i].name.clone().unwrap_or_else(|| "tool".into());
        tracing::warn!(
            target: deepseekcode_telemetry::SESSION,
            run_id = %deepseekcode_telemetry::current_run_id(),
            tool_call_id = %tool_id,
            assistant_index = a_idx,
            tool_index = i,
            "repairing missing assistant tool_calls before tool result"
        );
        messages[a_idx].tool_calls.push(LlmToolCall {
            id: tool_id,
            name,
            arguments: json!({}),
        });
    }
}

pub(crate) fn message_tool_calls(msg: &Message) -> Vec<LlmToolCall> {
    msg.parts
        .iter()
        .filter_map(|part| {
            if !matches!(part.kind, PartKind::ToolCall) {
                return None;
            }
            Some(LlmToolCall {
                id: part.tool_call_id.clone()?,
                name: part.tool_name.clone()?,
                arguments: deepseekcode_types::normalize_tool_arguments(
                    &part.tool_input.clone().unwrap_or(json!({})),
                ),
            })
        })
        .collect()
}

#[allow(dead_code)]
pub(crate) fn message_to_text(msg: &Message) -> String {
    msg.parts
        .iter()
        .filter_map(|part| match part.kind {
            PartKind::Text => part.text.clone(),
            PartKind::ToolCall => Some(format!(
                "Tool call {}: {}",
                part.tool_name.as_deref().unwrap_or("tool"),
                part.tool_input
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            )),
            PartKind::ToolResult => Some(format!(
                "Tool result {}: {}",
                part.tool_name.as_deref().unwrap_or("tool"),
                part.tool_output
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            )),
            PartKind::Reasoning => part.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn estimate_message_tokens(text: &str) -> u32 {
    // Issue-5: single source of truth — see `crate::token::estimate_str`.
    crate::token::estimate_str(text)
}

pub(crate) fn agent_step_limit(config: &DeepSeekCodeConfig, agent: &AgentKind) -> Option<usize> {
    let key = format!("{agent:?}").to_lowercase();
    let configured = config
        .agents
        .get(&key)
        .and_then(|a| a.steps.or(a.max_turns))
        .map(|limit| limit as usize);
    if configured.is_some() {
        return configured;
    }
    match agent {
        AgentKind::Plan => Some(50),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_llm::tool_call::LlmToolCall;

    #[test]
    fn tool_prefix_truncation_stays_bounded_and_stable() {
        let big = "x".repeat(100_000);
        let out = truncate_tool_content_for_prefix(&big);
        assert!(out.len() <= MAX_TOOL_PREFIX_BYTES);
        assert!(out.contains("truncated"));
        // Small content is byte-identical (no churn on warm prefix).
        assert_eq!(truncate_tool_content_for_prefix("ok"), "ok");
        // Same input → same output (cache key stability).
        assert_eq!(
            truncate_tool_content_for_prefix(&big),
            truncate_tool_content_for_prefix(&big)
        );
    }

    #[test]
    fn retry_predicate_only_strippable_reasoning_400s() {
        // P1-5: retry only when stripping reasoning can fix the failure.
        assert!(should_retry_without_reasoning(
            "400: unexpected reasoning_content with tools",
            true
        ));
        assert!(should_retry_without_reasoning(
            "Bad Request: tool_calls invalid with reasoning replay",
            true
        ));
        // No reasoning carried → retry would resend identical bytes.
        assert!(!should_retry_without_reasoning(
            "400: bad reasoning field",
            false
        ));
        // Overflow owns its recovery path; auth/quota/rate/network never retry.
        assert!(!should_retry_without_reasoning(
            "400: prompt exceeds the context window (reasoning?)",
            true
        ));
        assert!(!should_retry_without_reasoning(
            "401 Unauthorized: bad api key",
            true
        ));
        assert!(!should_retry_without_reasoning(
            "429 rate limited, reasoning quota exceeded",
            true
        ));
        // Non-shape errors pass through untouched.
        assert!(!should_retry_without_reasoning("500 internal error", true));
        assert!(!should_retry_without_reasoning("connection reset", true));
    }

    #[test]
    fn message_reasoning_joins_parts() {
        use deepseekcode_schema::message::{Message, MessagePart};
        let msg = Message {
            id: deepseekcode_types::MessageId::new(),
            session_id: deepseekcode_types::SessionId::new(),
            role: deepseekcode_schema::message::MessageRole::Assistant,
            parts: vec![MessagePart {
                kind: PartKind::Reasoning,
                text: Some("think".into()),
                tool_name: None,
                tool_input: None,
                tool_output: None,
                tool_call_id: None,
            }],
            seq: 0,
            created_at: chrono::Utc::now(),
        };
        assert_eq!(message_reasoning(&msg).as_deref(), Some("think"));
    }

    #[test]
    fn repairs_missing_tool_results_after_interrupt() {
        let mut messages = vec![
            LlmMessage {
                role: LlmRole::User,
                content: "continue".into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
            LlmMessage {
                role: LlmRole::Assistant,
                content: String::new(),
                tool_calls: vec![LlmToolCall {
                    id: "call_00_BPZiHCP7mg4Sczl7QKx89012".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "npm install"}),
                }],
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
        ];

        repair_llm_tool_chain(&mut messages);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].role, LlmRole::Tool);
        assert_eq!(
            messages[2].tool_call_id.as_deref(),
            Some("call_00_BPZiHCP7mg4Sczl7QKx89012")
        );
        assert!(messages[2].content.contains("interrupted"));
    }
}
