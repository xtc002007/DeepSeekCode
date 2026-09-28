//! DeepSeek API — OpenAI-compatible Chat Completions.
//! Docs: https://api-docs.deepseek.com/
//! V4 Flash model id: `deepseek-v4-flash`

use crate::event::LlmEvent;
use crate::providers::openai;
use crate::request::LlmRequest;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;

/// Official base URL is `https://api.deepseek.com` (no `/v1` suffix).
/// Chat endpoint is `POST {base}/chat/completions`.
/// See https://api-docs.deepseek.com/ — "Your First API Call".
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

fn resolve_base_url(base_url: Option<&str>) -> String {
    normalize_deepseek_base_url(base_url.unwrap_or(DEFAULT_BASE_URL))
}

/// Normalize DeepSeek base URLs.
///
/// - Strips stale `/v1` and accidental `/chat/completions` suffixes so both
///   `https://api.deepseek.com/v1` and `https://api.deepseek.com/v1/chat/completions`
///   resolve to `https://api.deepseek.com` (endpoint appends `/chat/completions`).
/// - Only applies to `*.deepseek.com` hosts; third-party bases pass through
///   (trimmed of trailing `/`).
pub fn normalize_deepseek_base_url(url: &str) -> String {
    let mut trimmed = url.trim().trim_end_matches('/').to_string();
    if !is_deepseek_host(&trimmed) {
        return trimmed;
    }
    // Peel accidental full endpoint suffixes repeatedly.
    loop {
        if let Some(s) = trimmed.strip_suffix("/chat/completions") {
            trimmed = s.trim_end_matches('/').to_string();
            continue;
        }
        if let Some(s) = trimmed.strip_suffix("/v1/chat/completions") {
            trimmed = s.trim_end_matches('/').to_string();
            continue;
        }
        if let Some(s) = trimmed.strip_suffix("/v1") {
            trimmed = s.trim_end_matches('/').to_string();
            continue;
        }
        break;
    }
    trimmed
}

fn is_deepseek_host(url: &str) -> bool {
    // Suffix-match the host, not substring: `evil-deepseek.com.evil.com`
    // must not be treated as official.
    #[allow(clippy::double_ended_iterator_last)]
    let host = url
        .split("://")
        .last()
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "deepseek.com" || host.ends_with(".deepseek.com")
}

/// Issue-2: real effort mapping. `disabled` (default) keeps agent/tool
/// loops cheap and prefix-stable; `enabled`/`high`/`max` opt into thinking.
/// The result is part of the cache lineage — changing effort intentionally
/// starts a new prefix.
pub fn normalize_deepseek_thinking(model: &str, effort: Option<&str>) -> &'static str {
    // Delegates to the shared normalizer (Issues 2+4): DeepSeek provider
    // paths are always DeepSeek, so `is_deepseek=true`.
    crate::request::normalize_thinking(model, effort, true)
}

/// Issue-4: `stream_options.include_usage` is only safe on the official
/// endpoint. Third-party DeepSeek-compatible gateways may reject the unknown
/// field with 400. Effective URL = explicit override → request → default.
pub fn is_official_deepseek_endpoint(
    request_base: Option<&str>,
    override_base: Option<&str>,
) -> bool {
    let effective = override_base.or(request_base).unwrap_or(DEFAULT_BASE_URL);
    is_deepseek_host(effective)
}

fn deepseek_extras(request: &LlmRequest, stream: bool, override_base: Option<&str>) -> Value {
    // `thinking` is DeepSeek-official-only: third-party gateways reject it
    // with 400 (doubling latency via bare retry). Gate both `thinking` and
    // `stream_options` to official endpoints.
    let official = is_official_deepseek_endpoint(request.base_url.as_deref(), override_base);
    if !official {
        if stream {
            return json!({});
        }
        return json!({});
    }
    let thinking_type = normalize_deepseek_thinking(&request.model, request.effort.as_deref());
    let thinking = json!({ "type": thinking_type });
    if stream {
        json!({
            "thinking": thinking,
            "stream_options": { "include_usage": true },
        })
    } else {
        json!({ "thinking": thinking })
    }
    // NOTE: `max_tokens`/`max_output_tokens` is intentionally omitted here.
    // The shared `build_openai_chat_body` already drops it for DeepSeek, so
    // the official endpoint uses its 384K ceiling instead of a truncated one.
}

pub async fn generate(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<crate::request::LlmResponse> {
    let api_key = api_key.filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        DeepSeekCodeError::Llm("DeepSeek API key not set (DEEPSEEK_API_KEY)".into())
    })?;
    let body = openai::build_chat_body(
        request,
        false,
        Some(&deepseek_extras(request, false, base_url)),
    );

    let resolved = resolve_base_url(base_url);
    let endpoint = openai::chat_endpoint(Some(resolved.as_str()));
    let response =
        match crate::http_client::post_json_with_fallback(http, &endpoint, &api_key, &body, true)
            .await
        {
            Ok(resp) => resp,
            Err(combined) => {
                // Last resort for VPN TUN fake-DNS: SNI-preserving pinned-IP
                // transport (DoH real IP, SNI stays the API hostname).
                if crate::sni_pinned::is_pinnable_host(&endpoint) {
                    match crate::sni_pinned::post_pinned(http, &endpoint, &api_key, &body).await {
                        Ok((status, bytes)) => {
                            return handle_pinned_generate_response(
                                http, request, &api_key, &resolved, status, &bytes,
                            )
                            .await
                        }
                        Err(pinned_err) => {
                            return Err(DeepSeekCodeError::Llm(map_transport_error(
                                &format!("{combined} || pinned: {pinned_err}"),
                                &endpoint,
                            )));
                        }
                    }
                }
                return Err(DeepSeekCodeError::Llm(map_transport_error(
                    &combined, &endpoint,
                )));
            }
        };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        // Issue-4 fallback for generate: gateways that reject DeepSeek-only
        // fields retry once with a bare OpenAI body.
        if status.as_u16() == 400 && mentions_deepseek_only_field(&text) {
            let plain = openai::build_chat_body(request, false, None);
            let retry = crate::http_client::post_json_with_fallback(
                http,
                &openai::chat_endpoint(Some(resolved.as_str())),
                &api_key,
                &plain,
                true,
            )
            .await
            .map_err(|combined| {
                let ep = openai::chat_endpoint(Some(resolved.as_str()));
                DeepSeekCodeError::Llm(map_transport_error(&combined, &ep))
            })?;
            if !retry.status().is_success() {
                let t = retry.text().await.unwrap_or_default();
                return Err(DeepSeekCodeError::Llm(t));
            }
            let value: Value = retry
                .json()
                .await
                .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;
            return openai::parse_message_response(&value);
        }
        return Err(DeepSeekCodeError::Llm(text));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;

    openai::parse_message_response(&value)
}

/// Handle a pinned-transport non-streaming result with the same semantics as
/// the reqwest path (400 DeepSeek-only-fields retry, then parse).
async fn handle_pinned_generate_response(
    http: &Client,
    request: &LlmRequest,
    api_key: &str,
    resolved: &str,
    status: u16,
    bytes: &bytes::Bytes,
) -> Result<crate::request::LlmResponse> {
    if status == 400 {
        let text = String::from_utf8_lossy(bytes).to_string();
        if mentions_deepseek_only_field(&text) {
            let plain = openai::build_chat_body(request, false, None);
            let endpoint = openai::chat_endpoint(Some(resolved));
            let (st, bd) = crate::sni_pinned::post_pinned(http, &endpoint, api_key, &plain)
                .await
                .map_err(DeepSeekCodeError::Llm)?;
            return crate::sni_pinned::parse_pinned_response(st, &bd);
        }
        return Err(DeepSeekCodeError::Llm(text));
    }
    crate::sni_pinned::parse_pinned_response(status, bytes)
}

/// Pinned-stream 400 retry: `stream_pinned` surfaces HTTP errors as
/// `"HTTP <code>: <body>"`. On a DeepSeek-only-fields 400, retry once with a
/// bare OpenAI body over the pinned transport.
async fn try_pinned_stream_400_retry(
    http: &Client,
    request: &LlmRequest,
    api_key: &str,
    resolved: &str,
    pinned_err: &str,
) -> std::result::Result<
    Option<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>>,
    DeepSeekCodeError,
> {
    let body = pinned_err.strip_prefix("HTTP 400:").unwrap_or("");
    if body.is_empty() || !mentions_deepseek_only_field(body) {
        return Ok(None);
    }
    let plain = openai::build_chat_body(request, true, None);
    let endpoint = openai::chat_endpoint(Some(resolved));
    match crate::sni_pinned::stream_pinned(http, &endpoint, api_key, &plain).await {
        Ok(events) => Ok(Some(events)),
        Err(e2) => match e2.strip_prefix("HTTP ") {
            Some(rest) => {
                let text = rest.split_once(':').map(|(_, b)| b.trim()).unwrap_or(&e2);
                Err(DeepSeekCodeError::Llm(text.to_string()))
            }
            None => Err(DeepSeekCodeError::Llm(e2)),
        },
    }
}

/// True when a 400 body complains about DeepSeek-only fields.
///
/// Structured check (not raw substring): a plain `"thinking"` substring also
/// matches legitimate bodies that merely MENTION thinking text, which used to
/// trigger a wasteful bare retry doubling latency. We require an
/// unknown-field-shaped error (`code`/`type`/`param` naming the field, or a
/// message pairing the field name with unknown/unrecognized/unsupported).
fn mentions_deepseek_only_field(text: &str) -> bool {
    const FIELDS: &[&str] = &["stream_options", "thinking", "reasoning_content"];
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        // Non-JSON bodies: only match explicit unknown-field phrasing.
        let lower = text.to_ascii_lowercase();
        return (lower.contains("unknown field")
            || lower.contains("unknown parameter")
            || lower.contains("unrecognized"))
            && FIELDS.iter().any(|f| lower.contains(f));
    };
    let err = &value["error"];
    let code = err.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let kind = err.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let param = err.get("param").and_then(|v| v.as_str()).unwrap_or("");
    let message = err
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
        .unwrap_or("");
    let names_field = [code, kind, param, message]
        .iter()
        .any(|s| FIELDS.iter().any(|f| s.to_ascii_lowercase().contains(f)));
    if !names_field {
        return false;
    }
    let lower = format!("{code} {kind} {param} {message}").to_ascii_lowercase();
    lower.contains("unknown")
        || lower.contains("unrecognized")
        || lower.contains("unsupported")
        || lower.contains("invalid_request_error")
}

/// Enrich transport fallback errors with the endpoint + actionable hints.
///
/// `post_json_with_fallback` already tried auto -> direct -> forced proxy, so
/// a final failure means neither VPN-ON (proxy) nor VPN-OFF (direct) worked.
/// Surface the full chain plus the proxy mode so users can check VPN/proxy.
pub(crate) fn map_transport_error(combined: &str, endpoint: &str) -> String {
    let proxy = crate::http_client::resolve_proxy_url();
    let mode = proxy
        .as_deref()
        .map(|p| format!("proxy {p}"))
        .unwrap_or_else(|| "direct (no proxy detected)".to_string());
    // Mask credentials in proxy URL for display.
    let mode = if let Some(at) = mode.rfind('@') {
        if let Some(scheme) = mode.find("://") {
            format!("{}://***@{}", &mode[..scheme], &mode[at + 1..])
        } else {
            mode
        }
    } else {
        mode
    };
    format!(
        "DeepSeek connect failed for url ({endpoint}): {combined}. Tried auto->direct->proxy [{mode}]; endpoint must be https://api.deepseek.com/chat/completions. Check VPN/proxy (HTTP/HTTPS_PROXY or system proxy), firewall, and DNS."
    )
}

pub async fn stream(
    http: &Client,
    request: &LlmRequest,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>> {
    let api_key = api_key.filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        DeepSeekCodeError::Llm("DeepSeek API key not set (DEEPSEEK_API_KEY)".into())
    })?;
    // Issue-4 fallback: if a gateway rejects `stream_options` (or `thinking`)
    // with 400, retry once without extras so custom proxies keep working.
    // The retry intentionally drops prefix-cache observability, not correctness.
    let body = openai::build_chat_body(
        request,
        true,
        Some(&deepseek_extras(request, true, base_url)),
    );

    let resolved = resolve_base_url(base_url);
    let endpoint = openai::chat_endpoint(Some(resolved.as_str()));
    let response =
        match crate::http_client::post_json_with_fallback(http, &endpoint, &api_key, &body, false)
            .await
        {
            Ok(resp) => resp,
            Err(combined) => {
                // Last resort: pinned-IP transport for VPN fake-DNS.
                if crate::sni_pinned::is_pinnable_host(&endpoint) {
                    match crate::sni_pinned::stream_pinned(http, &endpoint, &api_key, &body).await {
                        Ok(events) => return Ok(events),
                        Err(pinned_err) => {
                            if let Some(retry_events) = try_pinned_stream_400_retry(
                                http,
                                request,
                                &api_key,
                                &resolved,
                                &pinned_err,
                            )
                            .await?
                            {
                                return Ok(retry_events);
                            }
                            return Err(DeepSeekCodeError::Llm(map_transport_error(
                                &format!("{combined} || pinned: {pinned_err}"),
                                &endpoint,
                            )));
                        }
                    }
                }
                return Err(DeepSeekCodeError::Llm(map_transport_error(
                    &combined, &endpoint,
                )));
            }
        };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.as_u16() == 400 && mentions_deepseek_only_field(&text) {
            let plain = openai::build_chat_body(request, true, None);
            let retry = crate::http_client::post_json_with_fallback(
                http,
                &openai::chat_endpoint(Some(resolved.as_str())),
                &api_key,
                &plain,
                false,
            )
            .await
            .map_err(|combined| {
                let ep = openai::chat_endpoint(Some(resolved.as_str()));
                DeepSeekCodeError::Llm(map_transport_error(&combined, &ep))
            })?;
            if !retry.status().is_success() {
                let t = retry.text().await.unwrap_or_default();
                return Err(DeepSeekCodeError::Llm(t));
            }
            let byte_stream = retry.bytes_stream();
            let mapped = byte_stream
                .map(|chunk| chunk.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
                .scan(
                    (
                        String::new(),
                        HashMap::<usize, (String, String, String)>::new(),
                    ),
                    |state, chunk| {
                        let (buffer, tool_acc) = state;
                        match chunk {
                            Ok(bytes) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));
                                let mut events = Vec::new();
                                while let Some(pos) = buffer.find("\n\n") {
                                    let frame = buffer.drain(..=pos).collect::<String>();
                                    events.extend(openai::parse_sse_frame(&frame, tool_acc));
                                }
                                futures::future::ready(Some(events))
                            }
                            Err(e) => futures::future::ready(Some(vec![Err(e)])),
                        }
                    },
                )
                .flat_map(futures::stream::iter);
            return Ok(Box::pin(mapped));
        }
        return Err(DeepSeekCodeError::Llm(text));
    }

    let byte_stream = response.bytes_stream();
    let mapped = byte_stream
        .map(|chunk| chunk.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
        .scan(
            (
                String::new(),
                HashMap::<usize, (String, String, String)>::new(),
            ),
            |state, chunk| {
                let (buffer, tool_acc) = state;
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let mut events = Vec::new();
                        while let Some(pos) = buffer.find("\n\n") {
                            let frame = buffer.drain(..=pos).collect::<String>();
                            events.extend(openai::parse_sse_frame(&frame, tool_acc));
                        }
                        futures::future::ready(Some(events))
                    }
                    Err(e) => futures::future::ready(Some(vec![Err(e)])),
                }
            },
        )
        .flat_map(futures::stream::iter);

    Ok(Box::pin(mapped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_url_has_no_v1_suffix() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.deepseek.com");
        assert_eq!(
            crate::providers::openai::chat_endpoint(Some(DEFAULT_BASE_URL)),
            "https://api.deepseek.com/chat/completions"
        );
    }

    #[test]
    fn stale_v1_suffix_is_normalized() {
        assert_eq!(
            normalize_deepseek_base_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            normalize_deepseek_base_url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            crate::providers::openai::chat_endpoint(Some(
                normalize_deepseek_base_url("https://api.deepseek.com/v1").as_str()
            )),
            "https://api.deepseek.com/chat/completions"
        );
        // Non-DeepSeek URLs are untouched.
        assert_eq!(
            normalize_deepseek_base_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
    }
}
