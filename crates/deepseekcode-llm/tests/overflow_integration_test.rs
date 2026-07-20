use deepseekcode_llm::is_context_overflow;

/// Provider error bodies that should trigger context-overflow recovery.
const OVERFLOW_BODIES: &[&str] = &[
    r#"{"error":{"message":"prompt is too long: 200001 tokens > 200000 maximum","type":"invalid_request_error"}}"#,
    r#"{"error":{"message":"context_length_exceeded","type":"invalid_request_error"}}"#,
    r#"{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130000 tokens."}"#,
    r#"{"error":{"message":"input token count exceeds the maximum allowed for this model","type":"tokens"}}"#,
    r#"{"error":{"message":"request entity too large: context window exceeds limit","type":"server_error"}}"#,
    r#"{"error":{"message":"model_context_window_exceeded: reduce the length of the messages","type":"invalid_request_error"}}"#,
];

const NON_OVERFLOW_BODIES: &[&str] = &[
    r#"{"error":{"message":"rate limit exceeded","type":"rate_limit_error"}}"#,
    r#"{"error":{"message":"invalid api key","type":"authentication_error"}}"#,
    r#"{"error":{"message":"server overloaded","type":"server_error"}}"#,
];

fn extract_error_message(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("valid json");
    value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or(body)
        .to_string()
}

#[test]
fn provider_error_bodies_detect_context_overflow() {
    for body in OVERFLOW_BODIES {
        let message = extract_error_message(body);
        assert!(
            is_context_overflow(&message),
            "expected overflow for body: {body}"
        );
    }
}

#[test]
fn provider_error_bodies_ignore_non_overflow_failures() {
    for body in NON_OVERFLOW_BODIES {
        let message = extract_error_message(body);
        assert!(
            !is_context_overflow(&message),
            "expected non-overflow for body: {body}"
        );
    }
}

#[test]
fn anthropic_style_overflow_message() {
    let message = "prompt is too long: 200k tokens exceeds the maximum context length is 200000";
    assert!(is_context_overflow(message));
}

#[test]
fn openai_413_context_message() {
    let message = "Error 413: context length exceeded for this deployment";
    assert!(is_context_overflow(message));
}
