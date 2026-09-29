pub mod axis;
pub mod client;
pub mod event;
pub mod http_client;
pub mod overflow;
pub mod providers;
pub mod request;
pub mod sni_pinned;
pub mod tool_call;

pub use client::LlmClient;
pub use event::LlmEvent;
pub use overflow::is_context_overflow;
pub use providers::deepseek::DEFAULT_BASE_URL as DEEPSEEK_DEFAULT_BASE_URL;
pub use request::{LlmMessage, LlmRequest, LlmResponse, LlmRole, TokenUsage};
pub use tool_call::LlmToolCall;

/// Host-desensitized endpoint for logs: keeps `scheme://host/path`, strips
/// query/fragment/userinfo, lowercases host, caps length. Never emits keys.
pub fn redact_endpoint_for_log(endpoint: &str) -> String {
    let raw = endpoint.trim();
    if raw.is_empty() {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "llm endpoint empty; using unknown host for log"
        );
        return "unknown".to_string();
    }
    if raw.len() > 512 {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            endpoint_len = raw.len(),
            "llm endpoint overlong; truncating host for log"
        );
    }
    // Strip scheme, userinfo, path/query to isolate host (suffix-safe).
    let without_scheme = raw.split("://").last().unwrap_or(raw);
    let host_port = without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = host_port
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let host = if host.is_empty() {
        "unknown"
    } else {
        host.as_str()
    };
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        endpoint_host = %host,
        "llm endpoint redacted for log"
    );
    host.to_string()
}

/// Crate-level lifecycle log (product-grade run correlation).
pub fn log_crate_loaded() {
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        crate_name = "deepseekcode-llm",
        "llm crate loaded"
    );
}
