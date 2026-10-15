use code_plus_plus_types::{CodePlusPlusError, Result};
use reqwest::RequestBuilder;

pub trait Auth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder;
}

pub struct BearerAuth;

impl Auth for BearerAuth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder {
        if api_key.trim().is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "bearer",
                error = "empty api key",
                "llm auth failed: bearer key missing"
            );
        } else {
            tracing::debug!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "bearer",
                key_len = api_key.len(),
                "llm bearer auth applied"
            );
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "bearer",
                "llm auth applied"
            );
        }
        if api_key.len() > 4096 {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "bearer",
                key_len = api_key.len(),
                "llm auth key overlong"
            );
        }
        builder.bearer_auth(api_key)
    }
}

pub struct ApiKeyAuth;

impl Auth for ApiKeyAuth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder {
        if api_key.trim().is_empty() {
            tracing::error!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "x-api-key",
                error = "empty api key",
                "llm auth failed: x-api-key missing"
            );
        } else {
            tracing::debug!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "x-api-key",
                key_len = api_key.len(),
                "llm apikey auth applied"
            );
            tracing::info!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "x-api-key",
                "llm auth applied"
            );
        }
        if api_key.len() > 4096 {
            tracing::warn!(
                target: code_plus_plus_telemetry::LLM,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                auth = "x-api-key",
                key_len = api_key.len(),
                "llm auth key overlong"
            );
        }
        builder.header("x-api-key", api_key)
    }
}

/// Stub for future AWS Bedrock SigV4 signing.
/// Fail-closed by construction: no `Auth` impl exists, so an unsigned request
/// cannot be built via the `Auth` path at compile time. Callers must use
/// `apply_or_error`, which returns `Err` without panicking.
pub struct SigV4Auth;

impl SigV4Auth {
    pub fn unsupported() -> CodePlusPlusError {
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            auth = "sigv4",
            error = "unsupported",
            "llm auth failed: sigv4 unsupported"
        );
        CodePlusPlusError::Llm("SigV4 authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _api_key: &str,
    ) -> Result<RequestBuilder> {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            auth = "sigv4",
            "llm auth rejected: sigv4 fail-closed"
        );
        Err(Self::unsupported())
    }
}

/// Stub for future OAuth bearer refresh flows (fail-closed, see above).
/// No `Auth` impl by design — use `apply_or_error`.
pub struct OAuthAuth;

impl OAuthAuth {
    pub fn unsupported() -> CodePlusPlusError {
        tracing::error!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            auth = "oauth",
            error = "unsupported",
            "llm auth failed: oauth unsupported"
        );
        CodePlusPlusError::Llm("OAuth authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _token: &str,
    ) -> Result<RequestBuilder> {
        tracing::warn!(
            target: code_plus_plus_telemetry::LLM,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            auth = "oauth",
            "llm auth rejected: oauth fail-closed"
        );
        Err(Self::unsupported())
    }
}
