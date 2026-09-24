use crate::event::LlmEvent;
use crate::providers::{anthropic, deepseek, openai, openrouter};
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
        // Proxy-aware shared client: respects env proxy + OS system proxy
        // (requires reqwest `system-proxy` feature) so VPN ON/OFF both work.
        // Transport-level VPN fallback (direct <-> proxy) happens per-request
        // in `http_client::post_json_with_fallback`.
        Self {
            http: crate::http_client::build_auto_client(),
        }
    }

    pub async fn generate(
        &self,
        request: &LlmRequest,
        api_key: Option<String>,
    ) -> Result<LlmResponse> {
        // Validate early so unknown/empty providers are 400-class, not 500;
        // timeouts: non-stream 300s per-request, streams 10-min total cap with
        // runner idle watchdog (P0-5).
        request
            .validate()
            .map_err(|e| DeepSeekCodeError::InvalidInput(format!("invalid llm request: {e}")))?;
        match request.provider.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" => {
                openai::generate(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::generate(&self.http, request, api_key).await,
            "openrouter" => openrouter::generate(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::generate(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            other => Err(DeepSeekCodeError::InvalidInput(format!(
                "unsupported provider: {other}"
            ))),
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
        request
            .validate()
            .map_err(|e| DeepSeekCodeError::InvalidInput(format!("invalid llm request: {e}")))?;
        let result = match request.provider.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" => {
                openai::stream(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::stream(&self.http, request, api_key).await,
            "openrouter" => openrouter::stream(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::stream(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            other => Err(DeepSeekCodeError::InvalidInput(format!(
                "unsupported provider: {other}"
            ))),
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
