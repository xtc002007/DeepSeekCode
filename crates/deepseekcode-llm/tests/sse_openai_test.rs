use deepseekcode_llm::providers::openai;
use deepseekcode_llm::{LlmEvent, LlmMessage, LlmRequest, LlmRole};
use futures::StreamExt;
use reqwest::Client;
use std::collections::HashMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn openai_sse_stream_emits_text_and_completion() {
    let server = MockServer::start().await;
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let http = Client::new();
    let base = format!("{}/v1", server.uri());
    let request = LlmRequest {
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        system: "You are helpful.".into(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "Hi".into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }],
        tools: vec![],
        base_url: Some(base.clone()),
    };

    let mut stream = openai::stream(&http, &request, Some("test-key".into()), Some(&base))
        .await
        .expect("stream should open");

    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event.expect("event ok") {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::Completed => completed = true,
            other => panic!("unexpected event: {other:?}"),
        }
    }

    assert_eq!(text, "Hello world");
    assert!(completed);
}

#[tokio::test]
async fn openai_sse_stream_emits_tool_call_deltas() {
    let server = MockServer::start().await;
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_abc\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\"\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"README.md\\\"}\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let http = Client::new();
    let base = format!("{}/v1", server.uri());
    let request = LlmRequest {
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        system: "You are helpful.".into(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "Read README".into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }],
        tools: vec![],
        base_url: Some(base.clone()),
    };

    let mut stream = openai::stream(&http, &request, Some("test-key".into()), Some(&base))
        .await
        .expect("stream should open");

    let mut tool_deltas = Vec::new();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event.expect("event ok") {
            LlmEvent::ToolCallDelta { name, arguments, .. } => {
                tool_deltas.push((name, arguments));
            }
            LlmEvent::Completed => completed = true,
            other => panic!("unexpected event: {other:?}"),
        }
    }

    assert!(!tool_deltas.is_empty());
    assert_eq!(tool_deltas[0].0, "read");
    assert!(tool_deltas.iter().any(|(_, args)| args.contains("README")));
    assert!(completed);
}

#[tokio::test]
async fn openai_sse_parse_frame_handles_reasoning_delta() {
    let frame = concat!(
        "data: ",
        "{\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"},\"index\":0}]}\n\n",
    );
    let mut acc = HashMap::new();
    let events = deepseekcode_llm::axis::framing::parse_openai_sse_frame(frame, &mut acc);
    assert!(events.iter().any(|e| matches!(
        e,
        Ok(LlmEvent::ReasoningDelta(s)) if s == "think"
    )));
}
