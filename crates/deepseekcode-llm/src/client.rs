use crate::providers::{anthropic, deepseek, openai, openrouter};
use crate::event::LlmEvent;
use crate::request::{LlmRequest, LlmResponse};
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

pub struct LlmClient {
    http: Client,
}

impl LlmClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
        }
    }

    pub async fn generate(
        &mut self,
        request: &LlmRequest,
        api_key: Option<String>,
    ) -> Result<LlmResponse> {
        match request.provider.as_str() {
            "openai" | "openai-compatible" => {
                openai::generate(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::generate(&self.http, request, api_key).await,
            "openrouter" => openrouter::generate(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::generate(
                    &self.http,
                    request,
                    api_key,
                    request.base_url.as_deref(),
                )
                .await
            }
            other => Err(DeepSeekCodeError::Llm(format!("unsupported provider: {other}"))),
        }
    }

    pub async fn stream(
        &self,
        request: &LlmRequest,
        api_key: Option<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
        let start = std::time::Instant::now();
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            provider = %request.provider,
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            "llm stream start"
        );
        let result = match request.provider.as_str() {
            "openai" | "openai-compatible" => {
                openai::stream(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::stream(&self.http, request, api_key).await,
            "openrouter" => openrouter::stream(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::stream(
                    &self.http,
                    request,
                    api_key,
                    request.base_url.as_deref(),
                )
                .await
            }
            other => Err(DeepSeekCodeError::Llm(format!("unsupported provider: {other}"))),
        };
        match &result {
            Ok(_) => tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                provider = %request.provider,
                model = %request.model,
                duration_ms = start.elapsed().as_millis(),
                "llm stream opened"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::LLM,
                provider = %request.provider,
                model = %request.model,
                duration_ms = start.elapsed().as_millis(),
                error = %e,
                "llm stream failed"
            ),
        }
        result
    }
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}
