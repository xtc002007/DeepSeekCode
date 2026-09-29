use deepseekcode_types::Result;

pub trait Endpoint {
    fn resolve(
        &self,
        base_url: Option<&str>,
    ) -> Result<String>;
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

fn validate_base_url(base: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = base else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 512 || raw.contains([' ', '\0', '\n', '\r']) {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            base_len = raw.len(),
            "llm endpoint base_url malformed; rejecting (no silent fallback)"
        );
        return Err(deepseekcode_types::DeepSeekCodeError::InvalidInput(
            "invalid llm base_url: must be http(s) URL <=512 chars without whitespace".into(),
        ));
    }
    let lower = raw.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            error = "scheme must be http(s)",
            "llm endpoint base_url scheme invalid; rejecting (no silent fallback)"
        );
        return Err(deepseekcode_types::DeepSeekCodeError::InvalidInput(
            "invalid llm base_url: scheme must be http(s)://".into(),
        ));
    }
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        base_host = %crate::redact_endpoint_for_log(raw),
        "llm endpoint base_url validated"
    );
    Ok(Some(raw.trim_end_matches('/').to_string()))
}

impl Endpoint for OpenAiChatEndpoint {
    fn resolve(
        &self,
        base_url: Option<&str>,
    ) -> Result<String> {
        match validate_base_url(base_url)? {
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
                    return Err(deepseekcode_types::DeepSeekCodeError::InvalidInput(
                        "llm endpoint resolve produced empty url".into(),
                    ));
                }
                Ok(endpoint)
            }
            None => {
                let fallback = "https://api.openai.com/v1/chat/completions".to_string();
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                    "llm endpoint selected: openai default"
                );
                Ok(fallback)
            }
        }
    }
}

pub struct AnthropicMessagesEndpoint;

impl Endpoint for AnthropicMessagesEndpoint {
    fn resolve(
        &self,
        base_url: Option<&str>,
    ) -> Result<String> {
        match validate_base_url(base_url)? {
            Some(valid) => {
                let endpoint = join_endpoint(&valid, "/messages");
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&endpoint),
                    "llm endpoint selected: anthropic custom base"
                );
                if endpoint.trim().is_empty() {
                    return Err(deepseekcode_types::DeepSeekCodeError::InvalidInput(
                        "llm endpoint resolve produced empty url".into(),
                    ));
                }
                Ok(endpoint)
            }
            None => {
                let fallback = "https://api.anthropic.com/v1/messages".to_string();
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(&fallback),
                    "llm endpoint selected: anthropic default"
                );
                Ok(fallback)
            }
        }
    }
}
