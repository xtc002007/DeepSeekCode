use deepseekcode_types::{DeepSeekCodeError, Result};
use reqwest::RequestBuilder;

pub trait Auth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder;
}

pub struct BearerAuth;

impl Auth for BearerAuth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder {
        if api_key.trim().is_empty() {
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "bearer",
                error = "empty api key",
                "llm auth failed: bearer key missing"
            );
        } else {
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "bearer",
                key_len = api_key.len(),
                "llm bearer auth applied"
            );
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "bearer",
                "llm auth applied"
            );
        }
        if api_key.len() > 4096 {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
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
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "x-api-key",
                error = "empty api key",
                "llm auth failed: x-api-key missing"
            );
        } else {
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "x-api-key",
                key_len = api_key.len(),
                "llm apikey auth applied"
            );
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "x-api-key",
                "llm auth applied"
            );
        }
        if api_key.len() > 4096 {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                auth = "x-api-key",
                key_len = api_key.len(),
                "llm auth key overlong"
            );
        }
        builder.header("x-api-key", api_key)
    }
}

/// Stub for future AWS Bedrock SigV4 signing.
/// Fail-closed: `apply` MUST NOT emit an unsigned request. It logs and
/// returns the builder unusable by marking it; callers must use
/// `apply_or_error`. To enforce this, `apply` panics in all builds so a
/// silent unsigned request is impossible (P0-5). Use `apply_or_error`.
pub struct SigV4Auth;

impl Auth for SigV4Auth {
    fn apply(&self, _builder: RequestBuilder, _api_key: &str) -> RequestBuilder {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            error = "unsupported",
            "SigV4 authentication is not supported in this build"
        );
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            "llm auth fail-closed panic imminent"
        );
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            "llm sigv4 apply rejected"
        );
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            "llm sigv4 unsupported path"
        );
        panic!("SigV4Auth::apply must not be used; use apply_or_error (fail-closed)");
    }
}

impl SigV4Auth {
    pub fn unsupported() -> DeepSeekCodeError {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            error = "unsupported",
            "llm auth failed: sigv4 unsupported"
        );
        DeepSeekCodeError::Llm("SigV4 authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _api_key: &str,
    ) -> Result<RequestBuilder> {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "sigv4",
            "llm auth rejected: sigv4 fail-closed"
        );
        Err(Self::unsupported())
    }
}

/// Stub for future OAuth bearer refresh flows (fail-closed, see above).
pub struct OAuthAuth;

impl Auth for OAuthAuth {
    fn apply(&self, _builder: RequestBuilder, _token: &str) -> RequestBuilder {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            error = "unsupported",
            "OAuth authentication is not supported in this build"
        );
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            "llm auth fail-closed panic imminent"
        );
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            "llm oauth apply rejected"
        );
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            "llm oauth unsupported path"
        );
        panic!("OAuthAuth::apply must not be used; use apply_or_error (fail-closed)");
    }
}

impl OAuthAuth {
    pub fn unsupported() -> DeepSeekCodeError {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            error = "unsupported",
            "llm auth failed: oauth unsupported"
        );
        DeepSeekCodeError::Llm("OAuth authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _token: &str,
    ) -> Result<RequestBuilder> {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            auth = "oauth",
            "llm auth rejected: oauth fail-closed"
        );
        Err(Self::unsupported())
    }
}
