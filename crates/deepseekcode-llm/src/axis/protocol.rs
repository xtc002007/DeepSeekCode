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

pub fn build_openai_chat_body(request: &LlmRequest, stream: bool) -> Value {
    // DeepSeek automatic prefix cache requires a byte-stable prefix:
    // system first, then append-only history, tools sorted by name.
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
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".into()),
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
            // Cache-aware reasoning replay (DeepSeek-only): when tools are
            // present, replay the prior turn verbatim so the prefix hash
            // stays warm. Without tools DeepSeek ignores the field, so omit
            // it there to save tokens.
            if is_deepseek && !request.tools.is_empty() {
                if let Some(rc) = m.reasoning_content.as_deref().filter(|s| !s.is_empty()) {
                    msg["reasoning_content"] = json!(rc);
                }
            }
            messages.push(msg);
        } else {
            let mut msg = json!({ "role": role, "content": m.content });
            // Text-only assistant turns may still carry reasoning for cache
            // continuity; replay it under the same DeepSeek+tools gate.
            if m.role == crate::request::LlmRole::Assistant
                && is_deepseek
                && !request.tools.is_empty()
            {
                if let Some(rc) = m.reasoning_content.as_deref().filter(|s| !s.is_empty()) {
                    msg["reasoning_content"] = json!(rc);
                }
            }
            messages.push(msg);
        }
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });

    if !request.tools.is_empty() {
        // Sort by name: HashMap iteration is random and would break the
        // DeepSeek prefix on every turn.
        let mut tools = request.tools.clone();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
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
        }
    }
    body
}

pub fn merge_extra(body: &mut Value, extra: Option<&Value>) {
    if let Some(extra) = extra {
        if let Some(obj) = extra.as_object() {
            for (key, value) in obj {
                body[key] = value.clone();
            }
        }
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
