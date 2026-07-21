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
            messages.push(msg);
        } else {
            messages.push(json!({ "role": role, "content": m.content }));
        }
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });

    if !request.tools.is_empty() {
        body["tools"] = json!(request
            .tools
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
