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

/// Stub for future AWS Bedrock SigV4 signing.
pub struct SigV4Auth;

impl Auth for SigV4Auth {
    fn apply(&self, builder: RequestBuilder, _api_key: &str) -> RequestBuilder {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            "SigV4 authentication is not supported in this build"
        );
        builder
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

/// Stub for future OAuth bearer refresh flows.
pub struct OAuthAuth;

impl Auth for OAuthAuth {
    fn apply(&self, builder: RequestBuilder, _token: &str) -> RequestBuilder {
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            "OAuth authentication is not supported in this build"
        );
        builder
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
