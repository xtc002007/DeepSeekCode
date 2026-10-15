pub mod auth;
pub mod endpoint;
pub mod framing;
pub mod protocol;

pub use auth::{ApiKeyAuth, Auth, BearerAuth, OAuthAuth, SigV4Auth};
pub use endpoint::{AnthropicMessagesEndpoint, Endpoint, OpenAiChatEndpoint};
pub use framing::{Framing, SseFraming, SseSplitter};
pub use protocol::{merge_extra, OpenAiChatProtocol, Protocol};

/// Axis-layer lifecycle log (endpoint/protocol/auth routing observability).
pub fn log_axis_loaded() {
    tracing::info!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        layer = "axis",
        "llm axis loaded: endpoint/protocol/auth ready"
    );
    tracing::debug!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        layer = "axis",
        "llm axis debug probe"
    );
    tracing::warn!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        layer = "axis",
        "llm axis warn probe (no-op, coverage)"
    );
}

/// Axis routing error helper: protocol/auth/endpoint failures funnel here
/// so every axis error carries run correlation without leaking secrets.
pub fn log_axis_error(kind: &str, detail: &str) {
    tracing::error!(
        target: code_plus_plus_telemetry::LLM,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        kind,
        error_summary = %code_plus_plus_telemetry::summarize_prompt(detail, false),
        "llm axis error"
    );
}
