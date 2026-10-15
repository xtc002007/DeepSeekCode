pub mod anthropic;
pub mod deepseek;
pub mod openai;
pub mod openrouter;

/// Provider-layer routing log (which backend was selected, desensitized).
pub fn log_provider_route(provider: &str, model: &str, endpoint_host: &str) {
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider,
        model = %code_plus_plus_telemetry::redact(model),
        endpoint_host,
        "llm provider route selected"
    );
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider,
        model = %code_plus_plus_telemetry::redact(model),
        endpoint_host,
        "llm provider route debug"
    );
}

/// Provider-layer failure log (transport/status without secrets).
pub fn log_provider_error(provider: &str, model: &str, error: &str) {
    tracing::error!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider,
        model = %code_plus_plus_telemetry::redact(model),
        error_summary = %code_plus_plus_telemetry::summarize_prompt(error, false),
        "llm provider error"
    );
    tracing::warn!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        provider,
        model = %code_plus_plus_telemetry::redact(model),
        "llm provider warn probe"
    );
}
