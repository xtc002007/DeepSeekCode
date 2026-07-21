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

pub(crate) fn repair_llm_tool_chain(messages: &mut Vec<LlmMessage>) {
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

fn repair_missing_assistant_tool_calls(messages: &mut Vec<LlmMessage>) {
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
    if text.is_empty() {
        0
    } else {
        text.chars().count().div_ceil(3).max(1) as u32
    }
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
    fn repairs_missing_tool_results_after_interrupt() {
        let mut messages = vec![
            LlmMessage {
                role: LlmRole::User,
                content: "continue".into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
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
