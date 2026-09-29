//! Proxy-aware HTTP client for LLM providers.
//!
//! Background: the workspace used `reqwest` with `default-features = false`
//! and without the `system-proxy` feature, so Windows system proxies set by
//! VPN tools (Clash / V2RayN / TUN `reg` proxy) were silently ignored. With
//! VPN ON the app tried a direct TCP connect and failed with
//! `error sending request`; with VPN OFF direct worked. Users expect both to
//! work without restarting the app.
//!
//! This module:
//! - builds the shared client with system proxy + env proxy enabled
//!   (requires workspace `reqwest` features `system-proxy` + `socks`);
//! - resolves the effective proxy URL from env first, then Windows registry;
//! - retries a failed transport error across modes:
//!   auto (system+env) -> direct (no proxy) -> explicit proxy (force).
//!   The first success wins, so VPN ON/OFF both work even if the proxy is
//!   misconfigured or the bypass list sends China traffic direct.

use reqwest::{Client, Proxy, Response};
use serde_json::Value;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
#[allow(dead_code)]
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const NON_STREAM_TIMEOUT: Duration = Duration::from_secs(300);

fn base_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        // NOTE: no global read_timeout here. A 120s read_timeout kills
        // long-lived SSE streams with no data (long tool runs). Non-stream
        // calls set an explicit 300s total timeout per-request; streams rely
        // on the runner idle watchdog.
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(60))
        .user_agent("DeepSeekCode/1.0")
}

/// Shared client: respects env proxy (`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`,
/// `NO_PROXY`) **and** OS system proxy (Windows registry when the
/// `system-proxy` reqwest feature is enabled).
pub fn build_auto_client() -> Client {
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "llm auto client build start"
    );
    base_builder().build().unwrap_or_else(|e| {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            error = %e,
            "llm client builder failed; using default client"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            error = %e,
            "llm auto client build failed"
        );
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "llm auto client fallback to default"
        );
        Client::default()
    })
}

/// Direct client: bypasses every proxy. Used as fallback when the proxy path
/// fails (e.g. VPN proxy down but China endpoint reachable directly).
pub fn build_direct_client() -> Client {
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "llm direct client build start"
    );
    base_builder().no_proxy().build().unwrap_or_else(|e| {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            error = %e,
            "llm direct client builder failed; using default client"
        );
        tracing::error!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            error = %e,
            "llm direct client build failed"
        );
        tracing::info!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "llm direct client fallback to default"
        );
        Client::default()
    })
}

/// Explicit proxy client: forces ALL traffic through `proxy_url`, ignoring
/// bypass lists. Used as last resort when auto resolved to direct (bypass)
/// but direct is blocked (VPN global mode).
pub fn build_forced_proxy_client(proxy_url: &str) -> Option<Client> {
    let proxy = Proxy::all(proxy_url).ok()?;
    base_builder().proxy(proxy).build().ok()
}

/// Stream-capable client. Streams get a 10-minute total cap so a hung
/// connection can never block the runner forever (P0-5); per-chunk
/// idleness is enforced by the caller watchdog, not by reqwest read timeout
/// (which would kill legitimate long tool-run gaps).
pub fn build_stream_client() -> Client {
    build_auto_client()
}

pub const STREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(600);
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Resolve the effective proxy URL for diagnostics / forced retry.
///
/// Order: `HTTPS_PROXY`/`https_proxy` -> `HTTP_PROXY`/`http_proxy` ->
/// `ALL_PROXY`/`all_proxy` -> Windows system proxy (registry).
/// Returns `None` for direct. Results are cached 60s so the Windows
/// `reg query` helper (which spawns a thread + 5s recv timeout) never runs
/// on the hot path per request (P0-5).
pub fn resolve_proxy_url() -> Option<String> {
    use std::sync::OnceLock;
    use std::time::Instant;
    struct Cache {
        at: Instant,
        value: Option<String>,
    }
    static CACHE: OnceLock<std::sync::Mutex<Cache>> = OnceLock::new();
    let lock = CACHE.get_or_init(|| {
        std::sync::Mutex::new(Cache {
            at: Instant::now()
                .checked_sub(Duration::from_secs(61))
                .unwrap_or_else(Instant::now),
            value: None,
        })
    });
    if let Ok(guard) = lock.lock() {
        if guard.at.elapsed().as_secs() < 60 {
            return guard.value.clone();
        }
    }
    let fresh = resolve_proxy_url_uncached();
    if let Ok(mut guard) = lock.lock() {
        *guard = Cache {
            at: Instant::now(),
            value: fresh.clone(),
        };
    }
    fresh
}

fn resolve_proxy_url_uncached() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return normalize_proxy_url(&v);
            }
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(sys) = read_windows_system_proxy() {
        return Some(sys);
    }
    None
}

fn normalize_proxy_url(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.len() > 512 || t.contains([' ', '\0', '\n', '\r']) {
        return None;
    }
    let candidate = if t.contains("://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    // Validate scheme + host shape; reject garbage that would misdial.
    let lower = candidate.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks5://")
        || lower.starts_with("socks5h://"))
    {
        return None;
    }
    Some(candidate)
}

#[cfg(target_os = "windows")]
fn read_windows_system_proxy() -> Option<String> {
    // Blocking `reg` must not run on async workers: offload via blocking
    // task when inside a runtime, fall back to direct call otherwise.
    // Blocking `reg` must not stall an async worker: run it on a helper
    // thread and wait briefly (sync fn, called at most twice per fallback).
    let run_reg = |args: &[&str]| -> Option<String> {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let out = std::process::Command::new("reg")
                .args(&args_owned)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
            let _ = tx.send(out);
        });
        rx.recv_timeout(Duration::from_secs(5)).ok().flatten()
    };
    // `reg query` avoids a new `winreg` dependency and works on stock Windows.
    let text = run_reg(&[
        "query",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        "/v",
        "ProxyEnable",
    ])?;
    // `ProxyEnable    REG_DWORD    0x1`
    let enabled = text
        .lines()
        .any(|l| l.contains("ProxyEnable") && (l.contains("0x1") || l.trim_end().ends_with('1')));
    if !enabled {
        return None;
    }
    let text = run_reg(&[
        "query",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        "/v",
        "ProxyServer",
    ])?;
    // Value line looks like:
    // `    ProxyServer    REG_SZ    127.0.0.1:7890`
    // or per-protocol: `http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891`
    // `REG_EXPAND_SZ` (expandable strings) is also accepted.
    let line = text.lines().find(|l| l.contains("ProxyServer"))?;
    let after = line
        .split("REG_SZ")
        .nth(1)
        .or_else(|| line.split("REG_EXPAND_SZ").nth(1))?
        .trim();
    if after.is_empty() {
        return None;
    }
    // Prefer http=... else https=... else socks=... else bare first entry.
    let mut fallback: Option<String> = None;
    for part in after.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((proto, addr)) = part.split_once('=') {
            let proto = proto.trim().to_ascii_lowercase();
            let addr = addr.trim();
            if proto == "http" || proto == "https" {
                if let Some(url) = normalize_proxy_url(addr) {
                    return Some(url);
                }
                continue;
            }
            if fallback.is_none() {
                if proto.contains("socks") {
                    let url = if addr.contains("://") {
                        addr.to_string()
                    } else {
                        format!("socks5h://{addr}")
                    };
                    // Validate socks URL shape too.
                    if let Some(valid) = normalize_proxy_url(&url) {
                        fallback = Some(valid);
                    }
                } else if let Some(valid) = normalize_proxy_url(part) {
                    fallback = Some(valid);
                }
            }
        } else if let Some(valid) = normalize_proxy_url(part) {
            // Bare `127.0.0.1:7890` form.
            return Some(valid);
        }
    }
    fallback
}

#[cfg(not(target_os = "windows"))]
fn read_windows_system_proxy() -> Option<String> {
    None
}

/// Full source chain for a reqwest error (status + causes), e.g.
/// `error sending request ...: client error (Connect): tcp connect error:
/// connection refused (os error 111)`.
pub fn transport_debug(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut parts = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join(" | caused by: ")
}

pub fn is_transport_error(e: &reqwest::Error) -> bool {
    // Include mid-stream body/decode breaks so SSE disconnects can retry.
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

/// POST JSON with proxy-mode fallback.
///
/// Attempts in order:
///
/// 1. `primary` (shared auto client: system + env proxy),
/// 2. direct (no proxy),
/// 3. explicit forced proxy (if a proxy URL is known).
///
/// Returns the first HTTP response (even non-2xx; callers handle status).
/// Only transport errors trigger fallback; HTTP 4xx/5xx are returned directly.
#[allow(unused_assignments)]
pub async fn post_json_with_fallback(
    primary: &Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    non_stream_timeout: bool,
) -> Result<Response, String> {
    // 1. primary (auto)
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
        "llm transport primary attempt start"
    );
    let mut last_err: String;
    match send_one(primary, endpoint, bearer, body, non_stream_timeout).await {
        Ok(resp) => {
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                status = %resp.status(),
                "llm transport primary succeeded"
            );
            return Ok(resp);
        }
        Err(e) => {
            if !is_transport_error(&e) {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    error = %transport_debug(&e),
                    "llm transport non-retryable error"
                );
                return Err(transport_debug(&e));
            }
            last_err = format!("auto[{}]: {}", proxy_mode_label(), transport_debug(&e));
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                error_summary = %deepseekcode_telemetry::summarize_prompt(&last_err, false),
                "llm primary transport failed, trying direct"
            );
        }
    }

    // 2. direct (no proxy) — covers VPN proxy down / misconfigured, China
    // endpoint reachable directly.
    let direct = build_direct_client();
    match send_one(&direct, endpoint, bearer, body, non_stream_timeout).await {
        Ok(resp) => {
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                status = %resp.status(),
                "llm direct fallback succeeded"
            );
            tracing::debug!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                "llm direct fallback debug"
            );
            return Ok(resp);
        }
        Err(e) => {
            if !is_transport_error(&e) {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    error = %transport_debug(&e),
                    "llm direct non-retryable error"
                );
                return Err(transport_debug(&e));
            }
            let msg = format!("direct: {}", transport_debug(&e));
            last_err = format!("{last_err} || {msg}");
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                error_summary = %deepseekcode_telemetry::summarize_prompt(&msg, false),
                "llm direct fallback failed, trying explicit proxy"
            );
        }
    }

    // 3. explicit forced proxy — covers VPN global mode where auto resolved
    // to direct via bypass list but direct is blocked.
    if let Some(proxy_url) = resolve_proxy_url() {
        if let Some(client) = build_forced_proxy_client(&proxy_url) {
            match send_one(&client, endpoint, bearer, body, non_stream_timeout).await {
                Ok(resp) => {
                    tracing::info!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        proxy = %mask_proxy(&proxy_url),
                        status = %resp.status(),
                        "llm forced-proxy fallback succeeded"
                    );
                    return Ok(resp);
                }
                Err(e) => {
                    let msg = format!("proxy[{}]: {}", mask_proxy(&proxy_url), transport_debug(&e));
                    last_err = format!("{last_err} || {msg}");
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        error_summary = %deepseekcode_telemetry::summarize_prompt(&msg, false),
                        "llm forced-proxy fallback failed"
                    );
                }
            }
        }
    }

    tracing::error!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
        error_summary = %deepseekcode_telemetry::summarize_prompt(&last_err, false),
        "llm all transports failed"
    );
    Err(last_err)
}

async fn send_one(
    client: &Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    non_stream_timeout: bool,
) -> Result<Response, reqwest::Error> {
    send_one_with_auth(client, endpoint, bearer, body, non_stream_timeout, |b| b).await
}

fn retry_after_delay(resp: &Response, attempt: u32) -> Duration {
    // Honor `Retry-After` (seconds or HTTP date) when present; otherwise
    // exponential backoff 500ms<<attempt with up to 250ms jitter (P0-5).
    if let Some(raw) = resp.headers().get(reqwest::header::RETRY_AFTER) {
        if let Ok(s) = raw.to_str() {
            let s = s.trim();
            if let Ok(secs) = s.parse::<u64>() {
                return Duration::from_secs(secs.min(60));
            }
            // HTTP-date form: delay until that time (capped).
            if let Ok(date) = chrono::DateTime::parse_from_rfc2822(s) {
                let now = chrono::Utc::now();
                let delta = date.with_timezone(&chrono::Utc) - now;
                let secs = delta.num_seconds().clamp(0, 60) as u64;
                if secs > 0 {
                    return Duration::from_secs(secs);
                }
            }
        }
    }
    let base_ms = 500u64.saturating_mul(1 << attempt.min(4));
    // Cheap jitter without extra deps: nanos low bits.
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() % 250)
        .unwrap_or(0)) as u64;
    Duration::from_millis(base_ms + jitter)
}

async fn send_one_with_auth(
    client: &Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    non_stream_timeout: bool,
    auth: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<Response, reqwest::Error> {
    // Retry 429/5xx with Retry-After-aware backoff (max 3 attempts) before
    // surfacing; transport errors are handled by the outer fallback chain.
    let mut attempt = 0u32;
    loop {
        let base = client.post(endpoint).json(body);
        // Default bearer; custom auth closures (Anthropic x-api-key) override.
        let mut req = auth(base.bearer_auth(bearer));
        if non_stream_timeout {
            req = req.timeout(NON_STREAM_TIMEOUT);
        } else {
            // Streams: 10-minute total cap so a hung SSE can never block the
            // runner forever; idle gaps are owned by the runner watchdog.
            req = req.timeout(STREAM_TOTAL_TIMEOUT);
        }
        match req.send().await {
            Ok(resp) if is_retryable_status(resp.status()) && attempt < 2 => {
                attempt += 1;
                let backoff = retry_after_delay(&resp, attempt);
                tracing::warn!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    status = %resp.status(),
                    attempt,
                    backoff_ms = backoff.as_millis() as u64,
                    "llm retryable status, backing off"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    attempt,
                    "llm retry debug"
                );
                if resp.status().as_u16() == 429 {
                    tracing::info!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        "llm rate-limited; backing off"
                    );
                }
                tokio::time::sleep(backoff).await;
                continue;
            }
            Ok(resp) => {
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    status = %resp.status(),
                    "llm send succeeded"
                );
                return Ok(resp);
            }
            Err(e) => {
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    error = %transport_debug(&e),
                    "llm send transport error"
                );
                if !is_transport_error(&e) {
                    tracing::error!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        error = %transport_debug(&e),
                        "llm send non-retryable transport error"
                    );
                }
                return Err(e);
            }
        }
    }
}

/// POST with custom auth header (e.g. Anthropic `x-api-key`) + same
/// auto->direct->proxy fallback as bearer.
pub async fn post_json_with_fallback_auth(
    primary: &Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    non_stream_timeout: bool,
    auth: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + Clone,
) -> Result<Response, String> {
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
        "llm auth transport primary attempt start"
    );
    match send_one_with_auth(
        primary,
        endpoint,
        bearer,
        body,
        non_stream_timeout,
        auth.clone(),
    )
    .await
    {
        Ok(resp) => {
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                status = %resp.status(),
                "llm auth transport primary succeeded"
            );
            return Ok(resp);
        }
        Err(e) => {
            if !is_transport_error(&e) {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    error = %transport_debug(&e),
                    "llm auth transport non-retryable error"
                );
                return Err(transport_debug(&e));
            }
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                error_summary = %deepseekcode_telemetry::summarize_prompt(&transport_debug(&e), false),
                "llm primary transport failed, trying direct"
            );
        }
    }
    let direct = build_direct_client();
    match send_one_with_auth(
        &direct,
        endpoint,
        bearer,
        body,
        non_stream_timeout,
        auth.clone(),
    )
    .await
    {
        Ok(resp) => {
            tracing::info!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                status = %resp.status(),
                "llm auth direct fallback succeeded"
            );
            return Ok(resp);
        }
        Err(e) => {
            if !is_transport_error(&e) {
                tracing::error!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    error = %transport_debug(&e),
                    "llm auth direct non-retryable error"
                );
                return Err(transport_debug(&e));
            }
            tracing::warn!(
                target: deepseekcode_telemetry::LLM,
                run_id = %deepseekcode_telemetry::current_run_id(),
                endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                error_summary = %deepseekcode_telemetry::summarize_prompt(&transport_debug(&e), false),
                "llm auth direct fallback failed"
            );
        }
    }
    if let Some(proxy_url) = resolve_proxy_url() {
        if let Some(client) = build_forced_proxy_client(&proxy_url) {
            match send_one_with_auth(&client, endpoint, bearer, body, non_stream_timeout, auth)
                .await
            {
                Ok(resp) => {
                    tracing::info!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        status = %resp.status(),
                        "llm auth forced-proxy fallback succeeded"
                    );
                    return Ok(resp);
                }
                Err(e) => {
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                        error_summary = %deepseekcode_telemetry::summarize_prompt(&transport_debug(&e), false),
                        "llm auth forced-proxy fallback failed"
                    );
                    return Err(transport_debug(&e));
                }
            }
        }
    }
    tracing::error!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        endpoint_host = %crate::redact_endpoint_for_log(endpoint),
        "llm auth all transports failed"
    );
    Err("all transports failed".into())
}

/// Streaming POST with custom auth + fallback (no total timeout).
pub async fn post_json_stream_with_fallback_auth(
    primary: &Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    auth: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + Clone,
) -> Result<Response, String> {
    post_json_with_fallback_auth(primary, endpoint, bearer, body, false, auth).await
}

fn proxy_mode_label() -> &'static str {
    if resolve_proxy_url().is_some() {
        "proxy"
    } else {
        "direct"
    }
}

fn mask_proxy(url: &str) -> String {
    // Hide credentials: `http://user:pass@host:port` -> `http://***@host:port`.
    if let Some(at) = url.rfind('@') {
        if let Some(scheme) = url.find("://") {
            return format!("{}://***@{}", &url[..scheme], &url[at + 1..]);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_normalization_adds_scheme() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            normalize_proxy_url("http://127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            normalize_proxy_url("socks5h://127.0.0.1:7891").as_deref(),
            Some("socks5h://127.0.0.1:7891")
        );
        // P1-9: invalid inputs are rejected, not misdialed.
        assert_eq!(normalize_proxy_url(""), None);
        assert_eq!(normalize_proxy_url("http://has space/"), None);
    }

    #[test]
    fn clients_build_without_panic() {
        let _ = build_auto_client();
        let _ = build_direct_client();
    }
}
