use deepseekcode_llm::providers::anthropic;
use deepseekcode_llm::{LlmEvent, LlmMessage, LlmRequest, LlmRole};
use futures::StreamExt;
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn anthropic_sse_stream_emits_text_and_completion() {
    let server = MockServer::start().await;
    let sse_body = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" Claude\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
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
        provider: "anthropic".into(),
        model: "claude-sonnet-4-20250514".into(),
        system: "You are helpful.".into(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "Hi".into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }],
        tools: vec![],
        base_url: Some(base.clone()),
        effort: None,
        max_output_tokens: None,
    };

    let mut stream = anthropic::stream(&http, &request, Some("test-key".into()))
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

    assert_eq!(text, "Hello Claude");
    assert!(completed);
}

#[tokio::test]
async fn anthropic_sse_stream_emits_tool_use_block() {
    let server = MockServer::start().await;
    let sse_body = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01\",\"name\":\"read\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
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
        provider: "anthropic".into(),
        model: "claude-sonnet-4-20250514".into(),
        system: "You are helpful.".into(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "Read README".into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }],
        tools: vec![],
        base_url: Some(base.clone()),
        effort: None,
        max_output_tokens: None,
    };

    let mut stream = anthropic::stream(&http, &request, Some("test-key".into()))
        .await
        .expect("stream should open");

    let mut completed = false;
    while let Some(event) = stream.next().await {
        if matches!(event.expect("event ok"), LlmEvent::Completed) {
            completed = true;
        }
    }

    assert!(completed);
}
