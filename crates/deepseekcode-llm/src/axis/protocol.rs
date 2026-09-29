use crate::request::LlmRequest;
use serde_json::{json, Value};

pub trait Protocol {
    fn build_body(&self, request: &LlmRequest, stream: bool) -> Value;
}

pub struct OpenAiChatProtocol;

impl Protocol for OpenAiChatProtocol {
    fn build_body(&self, request: &LlmRequest, stream: bool) -> Value {
        build_openai_chat_body(request, stream)
    }
}

/// Whether `reasoning_content` is replayed for this request.
///
/// Explicit allowlist (previously two inline copies of the same gate, and
/// silent drops elsewhere): only DeepSeek honors the field, and only when
/// tools are present does replaying keep the prefix warm (without tools
/// DeepSeek ignores it, so omitting saves tokens). Anthropic carries
/// thinking in its own block types; OpenAI o1-style providers do not accept
/// this field — both drop it here by design, never silently by accident.
fn replays_reasoning(request: &LlmRequest, is_deepseek: bool) -> bool {
    is_deepseek && !request.tools.is_empty()
}

/// Cache-aware reasoning replay for assistant messages (see
/// [`replays_reasoning`]): replay the prior turn verbatim so the prefix hash
/// stays warm.
fn maybe_replay_reasoning(
    msg: &mut Value,
    m: &crate::request::LlmMessage,
    request: &LlmRequest,
    is_deepseek: bool,
) {
    if m.role != crate::request::LlmRole::Assistant || !replays_reasoning(request, is_deepseek) {
        return;
    }
    if let Some(rc) = m.reasoning_content.as_deref().filter(|s| !s.is_empty()) {
        msg["reasoning_content"] = json!(rc);
    }
}

pub fn build_openai_chat_body(request: &LlmRequest, stream: bool) -> Value {
    // DeepSeek automatic prefix cache requires a byte-stable prefix:
    // system first, then append-only history, tools sorted by name.
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = %request.provider,
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        stream,
        system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
        "llm protocol build_body start"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = %request.provider,
        model = %request.model,
        stream,
        "llm protocol build_body debug"
    );
    let is_deepseek = request.is_deepseek();
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": request.system })];
    for m in &request.messages {
        let role = match m.role {
            crate::request::LlmRole::User => "user",
            crate::request::LlmRole::Assistant => "assistant",
            crate::request::LlmRole::System => "system",
            crate::request::LlmRole::Tool => "tool",
        };

        if m.role == crate::request::LlmRole::Tool {
            let mut msg = json!({
                "role": role,
                "tool_call_id": m.tool_call_id,
                "content": m.content,
            });
            if let Some(name) = &m.name {
                msg["name"] = json!(name);
            }
            messages.push(msg);
        } else if m.role == crate::request::LlmRole::Assistant && !m.tool_calls.is_empty() {
            let tool_calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|c| {
                    let args = match serde_json::to_string(&c.arguments) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(
                                target: deepseekcode_telemetry::LLM,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                provider = %request.provider,
                                model = %request.model,
                                tool = %c.name,
                                error = %e,
                                "llm protocol tool arguments serialize failed; defaulting to empty object"
                            );
                            tracing::error!(
                                target: deepseekcode_telemetry::LLM,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                provider = %request.provider,
                                model = %request.model,
                                tool = %c.name,
                                error = %e,
                                "llm protocol error: tool args unserializable"
                            );
                            "{}".to_string()
                        }
                    };
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": args,
                        }
                    })
                })
                .collect();
            let mut msg = json!({ "role": role, "tool_calls": tool_calls });
            if !m.content.is_empty() {
                msg["content"] = json!(m.content);
            } else {
                msg["content"] = json!(null);
            }
            maybe_replay_reasoning(&mut msg, m, request, is_deepseek);
            messages.push(msg);
        } else {
            let mut msg = json!({ "role": role, "content": m.content });
            maybe_replay_reasoning(&mut msg, m, request, is_deepseek);
            messages.push(msg);
        }
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });

    if !request.tools.is_empty() {
        let mut tools = request.tools.clone();
        deepseekcode_schema::tool::sort_tool_definitions(&mut tools);
        body["tools"] = json!(tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            }))
            .collect::<Vec<_>>());
    }

    // Issue-3: DeepSeek main turns omit so the server uses its 384K ceiling
    // (no truncation, best quality). Summary requests (`tools` empty, e.g.
    // compaction checkpoints) DO send the cap so an 8192 summary cannot
    // balloon into six figures. Other providers send whenever positive.
    if let Some(n) = request.max_output_tokens {
        if n > 0 && (!is_deepseek || request.tools.is_empty()) {
            body["max_tokens"] = json!(n);
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                max_tokens = n,
                "llm protocol max_tokens applied"
            );
        } else {
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                max_output_tokens = ?request.max_output_tokens,
                "llm protocol max_tokens omitted (deepseek ceiling / legacy zero)"
            );
        }
        if n > 1_000_000 {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                max_tokens = n,
                "llm protocol output cap overflow/truncation risk"
            );
        }
    }
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider = %request.provider,
        model = %request.model,
        message_count = messages.len(),
        tool_count = request.tools.len(),
        "llm protocol build_body succeeded"
    );
    if body.get("model").is_none() {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = %request.provider,
            "llm protocol error: built body missing model"
        );
    }
    body
}

pub fn merge_extra(body: &mut Value, extra: Option<&Value>) {
    if let Some(extra) = extra {
        if let Some(obj) = extra.as_object() {
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                extra_keys = obj.len(),
                "llm protocol merge_extra applied"
            );
            for (key, value) in obj {
                body[key] = value.clone();
            }
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                extra_keys = obj.len(),
                "llm protocol extras merged"
            );
        } else {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                "llm protocol merge_extra got non-object; ignoring"
            );
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                "llm protocol error: extras not an object"
            );
        }
    } else {
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "llm protocol merge_extra: no extras"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{LlmMessage, LlmRequest, LlmRole};
    use serde_json::json;

    fn tool_def(name: &str) -> deepseekcode_schema::tool::ToolDefinition {
        deepseekcode_schema::tool::ToolDefinition {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn base_request(provider: &str) -> LlmRequest {
        LlmRequest {
            provider: provider.into(),
            model: "deepseek-chat".into(),
            system: "sys".into(),
            messages: vec![],
            tools: vec![],
            base_url: None,
            effort: None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn tools_serialize_sorted_for_prefix_stability() {
        let mut req = base_request("deepseek");
        req.tools = vec![tool_def("write"), tool_def("bash"), tool_def("read")];
        let body = build_openai_chat_body(&req, true);
        let names: Vec<String> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["bash", "read", "write"]);
    }

    #[test]
    fn reasoning_replayed_only_for_deepseek_with_tools() {
        let msg = LlmMessage {
            role: LlmRole::Assistant,
            content: "done".into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            reasoning_content: Some("think".into()),
        };
        let mut ds = base_request("deepseek");
        ds.messages = vec![msg.clone()];
        ds.tools = vec![tool_def("read")];
        let body = build_openai_chat_body(&ds, true);
        assert_eq!(body["messages"][1]["reasoning_content"], json!("think"));

        let mut ds_no_tools = base_request("deepseek");
        ds_no_tools.messages = vec![msg.clone()];
        let body = build_openai_chat_body(&ds_no_tools, true);
        assert!(body["messages"][1].get("reasoning_content").is_none());

        let mut oa = base_request("openai");
        oa.messages = vec![msg];
        oa.tools = vec![tool_def("read")];
        let body = build_openai_chat_body(&oa, true);
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn max_tokens_omitted_for_deepseek() {
        // Main turns (tools present) omit → 384K ceiling.
        let mut ds = base_request("deepseek");
        ds.tools = vec![tool_def("read")];
        ds.max_output_tokens = Some(4096);
        let body = build_openai_chat_body(&ds, false);
        assert!(body.get("max_tokens").is_none());

        // Issue-3: summary requests (tools empty) DO send the cap.
        let mut summary = base_request("deepseek");
        summary.max_output_tokens = Some(8192);
        let body = build_openai_chat_body(&summary, false);
        assert_eq!(body["max_tokens"], json!(8192));

        let mut oa = base_request("openai");
        oa.max_output_tokens = Some(4096);
        let body = build_openai_chat_body(&oa, false);
        assert_eq!(body["max_tokens"], json!(4096));
    }
}
