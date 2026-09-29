pub trait Endpoint {
    fn resolve(&self, base_url: Option<&str>) -> String;
}

pub struct OpenAiChatEndpoint;

fn join_endpoint(base: &str, suffix: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            suffix,
            "llm endpoint join with empty base; using suffix only"
        );
        return suffix.trim_start_matches('/').to_string();
    }
    // Avoid double suffix when the base already ends with it.
    if trimmed.ends_with(suffix) || trimmed.ends_with(&format!("{suffix}/")) {
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            base_host = %crate::redact_endpoint_for_log(trimmed),
            suffix,
            "llm endpoint base already contains suffix; not duplicating"
        );
        return trimmed.trim_end_matches('/').to_string();
    }
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        base_host = %crate::redact_endpoint_for_log(trimmed),
        suffix,
        "llm endpoint joined"
    );
    format!("{trimmed}{suffix}")
}

fn validate_base_url(base: Option<&str>) -> Option<String> {
    let raw = base?.trim();
    if raw.is_empty() || raw.len() > 512 || raw.contains([' ', '\0', '\n', '\r']) {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            base_host = %crate::redact_endpoint_for_log(raw),
            base_len = raw.len(),
            "llm endpoint base_url malformed; falling back to default"
        );
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            base_host = %crate::redact_endpoint_for_log(raw),
            error = "scheme must be http(s)",
            "llm endpoint base_url scheme invalid; falling back to default"
        );
        return None;
    }
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        base_host = %crate::redact_endpoint_for_log(raw),
        "llm endpoint base_url validated"
    );
    Some(raw.trim_end_matches('/').to_string())
}

impl Endpoint for OpenAiChatEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        match validate_base_url(base_url) {
            Some(valid) => {
                let endpoint = join_endpoint(&valid, "/chat/completions");
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&endpoint),
                    "llm endpoint selected: openai-chat custom base"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&endpoint),
                    "llm endpoint resolve debug"
                );
                if endpoint.trim().is_empty() {
                    tracing::error!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        "llm endpoint resolve produced empty url"
                    );
                }
                endpoint
            }
            None => {
                let fallback = "https://api.openai.com/v1/chat/completions".to_string();
                if base_url.is_some() {
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                        "llm endpoint falling back to openai default"
                    );
                } else {
                    tracing::info!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                        "llm endpoint selected: openai default"
                    );
                }
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                    "llm endpoint default debug"
                );
                fallback
            }
        }
    }
}

pub struct AnthropicMessagesEndpoint;

impl Endpoint for AnthropicMessagesEndpoint {
    fn resolve(&self, base_url: Option<&str>) -> String {
        match validate_base_url(base_url) {
            Some(valid) => {
                let endpoint = join_endpoint(&valid, "/messages");
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&endpoint),
                    "llm endpoint selected: anthropic custom base"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&endpoint),
                    "llm endpoint resolve debug"
                );
                if endpoint.trim().is_empty() {
                    tracing::error!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        "llm endpoint resolve produced empty url"
                    );
                }
                endpoint
            }
            None => {
                let fallback = "https://api.anthropic.com/v1/messages".to_string();
                if base_url.is_some() {
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                        "llm endpoint falling back to anthropic default"
                    );
                } else {
                    tracing::info!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                        "llm endpoint selected: anthropic default"
                    );
                }
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                    "llm endpoint default debug"
                );
                fallback
            }
        }
    }
}
