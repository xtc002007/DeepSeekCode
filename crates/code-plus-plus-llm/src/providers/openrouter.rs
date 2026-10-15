use crate::event::LlmEvent;
use crate::providers::openai;
use crate::request::LlmRequest;
use code_plus_plus_types::Result;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

fn require_key(api_key: Option<String>) -> Result<String> {
    match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => {
            tracing::debug!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                key_len = k.len(),
                "llm openrouter auth key present"
            );
            Ok(k)
        }
        None => {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                error = "OpenRouter API key not set",
                "llm auth failed"
            );
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                "llm openrouter missing key"
            );
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                "llm openrouter auth check failed"
            );
            Err(code_plus_plus_types::CodePlusPlusError::Llm(
                "OpenRouter API key not set".into(),
            ))
        }
    }
}

fn effective_base(request: &LlmRequest) -> &str {
    let base = request
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("https://openrouter.ai/api/v1");
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider = "openrouter",
        model = %request.model,
        endpoint_host = %crate::redact_endpoint_for_log(base),
        "llm openrouter base resolved"
    );
    base
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<crate::request::LlmResponse> {
    let start = std::time::Instant::now();
    let endpoint_host = crate::redact_endpoint_for_log(effective_base(request));
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider = "openrouter",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %code_plus_plus_telemetry::summarize_prompt(&request.system, false),
        "llm openrouter generate start"
    );
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider = "openrouter",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm openrouter generate debug"
    );
    crate::providers::log_provider_route("openrouter", &request.model, &endpoint_host);
    let api_key = match require_key(api_key) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                error = %e,
                "llm openrouter generate auth failed"
            );
            return Err(e);
        }
    };
    if let Err(e) = request.validate() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            provider = "openrouter",
            model = %request.model,
            error = %e,
            "llm openrouter generate validation failed"
        );
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            provider = "openrouter",
            model = %request.model,
            error = %e,
            "llm openrouter generate invalid request"
        );
        return Err(e);
    }
    match openai::generate(http, request, Some(api_key), Some(effective_base(request))).await {
        Ok(resp) => {
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                output_tokens = ?resp.usage.as_ref().and_then(|u| u.output_tokens),
                tool_call_count = resp.tool_calls.len(),
                "llm openrouter generate succeeded"
            );
            Ok(resp)
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                duration_ms = start.elapsed().as_millis() as u64,
                error_summary = %code_plus_plus_telemetry::summarize_prompt(&msg, false),
                "llm openrouter generate failed"
            );
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                error = %e,
                "llm openrouter generate error"
            );
            if crate::overflow::is_context_overflow(&msg) {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    provider = "openrouter",
                    model = %request.model,
                    "llm openrouter generate context overflow"
                );
            }
            crate::providers::log_provider_error("openrouter", &request.model, &msg);
            Err(e)
        }
    }
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let start = std::time::Instant::now();
    let endpoint_host = crate::redact_endpoint_for_log(effective_base(request));
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider = "openrouter",
        model = %request.model,
        endpoint_host = %endpoint_host,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        system_summary = %code_plus_plus_telemetry::summarize_prompt(&request.system, false),
        "llm openrouter stream start"
    );
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider = "openrouter",
        model = %request.model,
        endpoint_host = %endpoint_host,
        "llm openrouter stream debug"
    );
    crate::providers::log_provider_route("openrouter", &request.model, &endpoint_host);
    let api_key = match require_key(api_key) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                error = %e,
                "llm openrouter stream auth failed"
            );
            return Err(e);
        }
    };
    if let Err(e) = request.validate() {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            provider = "openrouter",
            model = %request.model,
            error = %e,
            "llm openrouter stream validation failed"
        );
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            provider = "openrouter",
            model = %request.model,
            error = %e,
            "llm openrouter stream invalid request"
        );
        return Err(e);
    }
    match openai::stream(http, request, Some(api_key), Some(effective_base(request))).await {
        Ok(s) => {
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                endpoint_host = %endpoint_host,
                duration_ms = start.elapsed().as_millis() as u64,
                "llm openrouter stream opened"
            );
            Ok(s)
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                duration_ms = start.elapsed().as_millis() as u64,
                error_summary = %code_plus_plus_telemetry::summarize_prompt(&msg, false),
                "llm openrouter stream failed"
            );
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                provider = "openrouter",
                model = %request.model,
                error = %e,
                "llm openrouter stream error"
            );
            if crate::overflow::is_context_overflow(&msg) {
                tracing::warn!(
                    target: code_plus_plus_telemetry::LLM,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    provider = "openrouter",
                    model = %request.model,
                    "llm openrouter stream context overflow"
                );
            }
            crate::providers::log_provider_error("openrouter", &request.model, &msg);
            Err(e)
        }
    }
}
