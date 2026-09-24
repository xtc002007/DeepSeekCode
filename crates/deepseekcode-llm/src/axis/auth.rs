use deepseekcode_types::DeepSeekCodeError;
use reqwest::RequestBuilder;

pub trait Auth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder;
}

pub struct BearerAuth;

impl Auth for BearerAuth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder {
        builder.bearer_auth(api_key)
    }
}

pub struct ApiKeyAuth;

impl Auth for ApiKeyAuth {
    fn apply(&self, builder: RequestBuilder, api_key: &str) -> RequestBuilder {
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
            "SigV4 authentication is not supported in this build"
        );
        panic!("SigV4Auth::apply must not be used; use apply_or_error (fail-closed)");
    }
}

impl SigV4Auth {
    pub fn unsupported() -> DeepSeekCodeError {
        DeepSeekCodeError::Llm("SigV4 authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _api_key: &str,
    ) -> Result<RequestBuilder, DeepSeekCodeError> {
        Err(Self::unsupported())
    }
}

/// Stub for future OAuth bearer refresh flows (fail-closed, see above).
pub struct OAuthAuth;

impl Auth for OAuthAuth {
    fn apply(&self, _builder: RequestBuilder, _token: &str) -> RequestBuilder {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            "OAuth authentication is not supported in this build"
        );
        panic!("OAuthAuth::apply must not be used; use apply_or_error (fail-closed)");
    }
}

impl OAuthAuth {
    pub fn unsupported() -> DeepSeekCodeError {
        DeepSeekCodeError::Llm("OAuth authentication is not supported in this build".into())
    }

    pub fn apply_or_error(
        &self,
        _builder: RequestBuilder,
        _token: &str,
    ) -> Result<RequestBuilder, DeepSeekCodeError> {
        Err(Self::unsupported())
    }
}
