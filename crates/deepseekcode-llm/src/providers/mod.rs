pub mod anthropic;
pub mod deepseek;
pub mod openai;
pub mod openrouter;

/// Provider-layer routing log (which backend was selected, desensitized).
pub fn log_provider_route(provider: &str, model: &str, endpoint_host: &str) {
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider,
        model = %deepseekcode_telemetry::redact(model),
        endpoint_host,
        "llm provider route selected"
    );
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider,
        model = %deepseekcode_telemetry::redact(model),
        endpoint_host,
        "llm provider route debug"
    );
}

/// Provider-layer failure log (transport/status without secrets).
pub fn log_provider_error(provider: &str, model: &str, error: &str) {
    tracing::error!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider,
        model = %deepseekcode_telemetry::redact(model),
        error_summary = %deepseekcode_telemetry::summarize_prompt(error, false),
        "llm provider error"
    );
    tracing::warn!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        provider,
        model = %deepseekcode_telemetry::redact(model),
        "llm provider warn probe"
    );
}
