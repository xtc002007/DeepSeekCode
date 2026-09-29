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
        let start = std::time::Instant::now();
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = %request.provider,
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
            "llm generate start"
        );
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = %request.provider,
            model = %request.model,
            "llm generate debug"
        );
        // Validate early so unknown/empty providers are 400-class, not 500;
        // timeouts: non-stream 300s per-request, streams 10-min total cap with
        // runner idle watchdog (P0-5).
        if let Err(e) = request.validate() {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                error = %e,
                "llm generate validation failed"
            );
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                error = %e,
                "llm generate invalid request"
            );
            return Err(DeepSeekCodeError::InvalidInput(format!(
                "invalid llm request: {e}"
            )));
        }
        let result = match request.provider.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" => {
                openai::generate(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::generate(&self.http, request, api_key).await,
            "openrouter" => openrouter::generate(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::generate(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            other => {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    error = %other,
                    "llm generate unsupported provider"
                );
                Err(DeepSeekCodeError::InvalidInput(format!(
                    "unsupported provider: {other}"
                )))
            }
        };
        match &result {
            Ok(resp) => tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                duration_ms = start.elapsed().as_millis() as u64,
                output_tokens = ?resp.usage.as_ref().and_then(|u| u.output_tokens),
                tool_call_count = resp.tool_calls.len(),
                "llm generate succeeded"
            ),
            Err(e) => tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "llm generate failed"
            ),
        }
        result
    }

    pub async fn stream(
        &self,
        request: &LlmRequest,
        api_key: Option<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
        let start = std::time::Instant::now();
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = %request.provider,
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            system_summary = %deepseekcode_telemetry::summarize_prompt(&request.system, false),
            "llm stream start"
        );
        tracing::debug!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            provider = %request.provider,
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            "llm stream debug"
        );
        if let Err(e) = request.validate() {
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                error = %e,
                "llm stream validation failed"
            );
            tracing::error!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider = %request.provider,
                model = %request.model,
                error = %e,
                "llm stream invalid request"
            );
            return Err(DeepSeekCodeError::InvalidInput(format!(
                "invalid llm request: {e}"
            )));
        }
        let result = match request.provider.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" => {
                openai::stream(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            "anthropic" => anthropic::stream(&self.http, request, api_key).await,
            "openrouter" => openrouter::stream(&self.http, request, api_key).await,
            "deepseek" => {
                deepseek::stream(&self.http, request, api_key, request.base_url.as_deref()).await
            }
            other => {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    error = %other,
                    "llm stream unsupported provider"
                );
                tracing::warn!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    "llm stream routing failed"
                );
                Err(DeepSeekCodeError::InvalidInput(format!(
                    "unsupported provider: {other}"
                )))
            }
        };
        match &result {
            Ok(_) => {
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "llm stream opened"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "llm stream debug opened"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    duration_ms = start.elapsed().as_millis() as u64,
                    error = %e,
                    "llm stream open warn"
                );
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider = %request.provider,
                    model = %request.model,
                    duration_ms = start.elapsed().as_millis() as u64,
                    error = %e,
                    "llm stream failed"
                );
            }
        }
        result
    }
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}
