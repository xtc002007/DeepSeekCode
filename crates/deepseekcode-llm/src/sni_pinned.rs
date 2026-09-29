//! SNI-preserving pinned-IP transport for VPN TUN fake-DNS environments.
//!
//! Symptom (observed on user machine with VPN TUN on):
//! - System DNS resolves `api.deepseek.com` to fake `198.18.0.x` (TUN fake-IP).
//! - The VPN exit's own DNS also yields a dead endpoint: TLS handshake EOF on
//!   auto (proxy), direct, and forced-proxy paths alike.
//! - DoH (`dns.google` via proxy) resolves the real CloudFront IP
//!   (`3.173.21.63`) and TLS to it succeeds on both proxy and direct paths.
//!
//! Standard `reqwest` cannot express "TCP dial IP X while TLS SNI stays
//! `api.deepseek.com`" (no `--resolve` equivalent), so this module builds a
//! small `hyper` + `hyper-rustls` client over a custom Tower connector:
//! - direct: `TcpStream` to a DoH-resolved real IP;
//! - proxy: `TcpStream` to the system/env HTTP proxy + manual `CONNECT
//!   <real-ip>:443`, then TLS over the tunnel with SNI = API hostname.
//!
//! Only used as a last resort for `*.deepseek.com` after the normal
//! `post_json_with_fallback` (auto -> direct -> proxy) fails with a
//! transport error. Normal users never pay the DoH cost.

use bytes::Bytes;
use http::{Request, Uri};
use http_body_util::{BodyExt, Full};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::event::LlmEvent;
use crate::request::LlmResponse;
use deepseekcode_types::{DeepSeekCodeError, Result};
use futures::{Stream, StreamExt};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const OPEN_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-attempt cap for every pinned IP try (direct and proxy): with up to 4
/// direct + 4 proxy attempts, the old 300s cap could stall the runner
/// ~40min. 30s per IP bounds the worst case to ~4min while slow-but-working
/// paths still succeed.
const PER_IP_TIMEOUT: Duration = Duration::from_secs(30);
const DOH_TTL: Duration = Duration::from_secs(600);
const MAX_IPS: usize = 4;

/// Only these hosts may use the pinned path (limits blast radius).
/// Suffix-match the host: substring `contains` would let
/// `evil-deepseek.com.evil.com` trigger the pinned bypass.
pub fn is_pinnable_host(endpoint: &str) -> bool {
    let host = host_of(endpoint).unwrap_or_default().to_ascii_lowercase();
    host == "deepseek.com" || host.ends_with(".deepseek.com")
}

/// True for non-routable IPv4 that a public API host must never resolve to:
/// TUN fake-DNS ranges (Clash `198.18.0.0/15`), CGNAT (`100.64.0.0/10`, also
/// used by some TUN stacks), RFC1918 private space, and link-local. Previous
/// code only covered `198.18/15`, so `10/8` and `100.64/10` fake answers from
/// other TUN setups slipped through to the pinned dial.
fn is_fake_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 169 && o[1] == 254)
                || (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        }
        // NOTE: DoH queries are A-only, so AAAA answers never arrive here;
        // IPv6-only networks remain unsupported by the pinned path (falls
        // back to the reqwest/system path via callers).
        IpAddr::V6(_) => false,
    }
}

type DohCache = tokio::sync::RwLock<HashMap<String, (Instant, Vec<IpAddr>)>>;

#[allow(clippy::type_complexity)]
fn doh_cache() -> &'static DohCache {
    static CACHE: OnceLock<DohCache> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

/// Resolve `host` to real IPs via DoH (JSON API), with a short TTL cache.
///
/// Tries `dns.google` then Cloudflare DNS-JSON, using the shared reqwest
/// client (which already handles proxy). Filters out TUN fake IPs.
pub async fn doh_resolve(
    http: &reqwest::Client,
    host: &str,
) -> std::result::Result<Vec<IpAddr>, String> {
    {
        let cache = doh_cache().read().await;
        if let Some((at, ips)) = cache.get(host) {
            if at.elapsed() < DOH_TTL && !ips.is_empty() {
                return Ok(ips.clone());
            }
        }
    }
    // Negative cache (60s): without it, an all-fake/all-failing answer
    // re-hammers DoH on every LLM call instead of failing fast.
    const NEG_TTL: Duration = Duration::from_secs(60);
    let neg_key = format!("{host}\0neg");
    {
        let cache = doh_cache().read().await;
        if let Some((at, ips)) = cache.get(&neg_key) {
            if at.elapsed() < NEG_TTL && ips.is_empty() {
                return Err(format!("DoH {host}: negative cache hit"));
            }
        }
    }
    let urls = [
        format!("https://dns.google/resolve?name={host}&type=A"),
        format!("https://cloudflare-dns.com/dns-query?name={host}&type=A"),
    ];
    let mut last_err = String::from("no DoH endpoint attempted");
    for url in &urls {
        let mut req = http.get(url).timeout(Duration::from_secs(20));
        if url.contains("cloudflare") {
            req = req.header("Accept", "application/dns-json");
        }
        match req.send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    last_err = format!("DoH {url} HTTP {}", resp.status());
                    continue;
                }
                match resp.json::<serde_json::Value>().await {
                    Ok(v) => {
                        let mut ips = Vec::new();
                        if let Some(arr) = v.get("Answer").and_then(|a| a.as_array()) {
                            for a in arr {
                                let is_a = a.get("type").and_then(|t| t.as_u64()) == Some(1);
                                if !is_a {
                                    continue;
                                }
                                if let Some(data) = a.get("data").and_then(|d| d.as_str()) {
                                    if let Ok(ip) = data.trim().parse::<IpAddr>() {
                                        if !is_fake_ip(&ip) {
                                            ips.push(ip);
                                        }
                                    }
                                }
                            }
                        }
                        if !ips.is_empty() {
                            ips.truncate(MAX_IPS);
                            doh_cache()
                                .write()
                                .await
                                .insert(host.to_string(), (Instant::now(), ips.clone()));
                            return Ok(ips);
                        }
                        last_err = format!("DoH {url} returned no usable A records");
                    }
                    Err(e) => last_err = format!("DoH {url} JSON error: {e}"),
                }
            }
            Err(e) => last_err = crate::http_client::transport_debug(&e),
        }
    }
    // Remember the failure briefly so consecutive turns fail fast instead of
    // re-issuing two DoH queries per LLM call.
    doh_cache()
        .write()
        .await
        .insert(neg_key, (Instant::now(), Vec::new()));
    Err(last_err)
}

/// System DNS result for `host` — used to decide whether pinning is needed.
/// Kept for diagnostics/tests; not on the hot path.
#[allow(dead_code)]
pub async fn system_addrs(host: &str, port: u16) -> Vec<IpAddr> {
    tokio::net::lookup_host((host, port))
        .await
        .map(|it| it.map(|s| s.ip()).collect())
        .unwrap_or_default()
}

/// Direct-TCP connector that dials a pinned IP regardless of the URI host.
/// TLS SNI is derived from the URI by `hyper-rustls`, so SNI stays correct.
#[derive(Clone)]
struct PinnedConnector {
    ip: IpAddr,
}

impl tower_service::Service<Uri> for PinnedConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = std::io::Result<TokioIo<TcpStream>>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let ip = self.ip;
        Box::pin(async move {
            let port = dst.port_u16().unwrap_or(443);
            let addr = SocketAddr::new(ip, port);
            let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "pinned connect timeout")
                })??;
            stream.set_nodelay(true).ok();
            Ok(TokioIo::new(stream))
        })
    }
}

#[derive(Clone, Debug)]
struct ProxyTarget {
    proxy_host: String,
    proxy_port: u16,
    proxy_auth: Option<String>,
    real_ip: IpAddr,
    real_port: u16,
    /// Origin hostname for the `Host` header: some proxies reject a literal
    /// IP there even though the CONNECT authority must stay `ip:port`.
    real_host: String,
}

/// Proxy-tunnel connector: TCP to the HTTP proxy, manual `CONNECT
/// <real-ip>:<port>`, then the tunnel is handed to TLS (SNI = URI host).
#[derive(Clone)]
struct ProxiedPinnedConnector {
    target: ProxyTarget,
}

impl tower_service::Service<Uri> for ProxiedPinnedConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = std::io::Result<TokioIo<TcpStream>>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _dst: Uri) -> Self::Future {
        let t = self.target.clone();
        Box::pin(async move {
            let mut stream = tokio::time::timeout(
                CONNECT_TIMEOUT,
                TcpStream::connect((t.proxy_host.as_str(), t.proxy_port)),
            )
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "proxy connect timeout")
            })??;
            stream.set_nodelay(true).ok();
            let mut req = format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}\r\nProxy-Connection: Keep-Alive\r\n",
                t.real_ip, t.real_port, t.real_host
            );
            if let Some(auth) = &t.proxy_auth {
                req.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
            }
            req.push_str("\r\n");
            stream.write_all(req.as_bytes()).await?;
            stream.flush().await?;
            let mut buf = vec![0u8; 4096];
            let mut acc: Vec<u8> = Vec::with_capacity(512);
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let n = stream.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    acc.extend_from_slice(&buf[..n]);
                    if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if acc.len() > 8192 {
                        break;
                    }
                }
                Ok::<_, std::io::Error>(())
            })
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "proxy CONNECT timeout")
            })??;
            let text = String::from_utf8_lossy(&acc);
            let status_line = text.lines().next().unwrap_or("").to_string();
            if !(status_line.contains(" 200") || status_line.contains(" 201")) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!("proxy CONNECT rejected: {status_line}"),
                ));
            }
            Ok(TokioIo::new(stream))
        })
    }
}

fn tls_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // rustls-native-certs 0.8 returns a `CertificateResult` struct
    // `{ certs, errors }` (not a `Result`).
    let native = rustls_native_certs::load_native_certs();
    for c in native.certs {
        let _ = roots.add(c);
    }
    if !native.errors.is_empty() {
        tracing::warn!(
            target: deepseekcode_telemetry::LLM,
            run_id = %deepseekcode_telemetry::current_run_id(),
            errors = native.errors.len(),
            "pinnedTLS: some native certs failed to load, webpki + loaded natives used"
        );
    }
    tracing::debug!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "llm pinned tls config built"
    );
    tracing::info!(
        target: deepseekcode_telemetry::LLM,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "llm pinned tls ready"
    );
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

type PinnedClient<C> = Client<HttpsConnector<C>, Full<Bytes>>;

/// Split `http://[user[:pass]@]host:port` into parts.
/// Scheme-aware default ports (http:80 https:443 socks:1080) instead of a
/// blanket 8080; reuses the workspace `base64` crate instead of hand-rolled.
fn parse_proxy(proxy_url: &str) -> Option<(String, u16, Option<String>)> {
    let (scheme, rest) = match proxy_url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_string(), proxy_url),
    };
    let default_port: u16 = match scheme.as_str() {
        "https" => 443,
        "socks5" | "socks5h" | "socks" => 1080,
        _ => 80,
    };
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a.to_string()), h),
        None => (None, rest),
    };
    // Strip trailing path if any.
    let hostport = hostport.split('/').next().unwrap_or(hostport);
    // IPv6 `[::1]:port` — keep simple: split last ':'.
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h.trim_matches(&['[', ']'][..]).to_string(), port),
            Err(_) => (hostport.to_string(), default_port),
        },
        None => (hostport.to_string(), default_port),
    };
    if host.is_empty() {
        return None;
    }
    let auth_b64 = auth.map(|a| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(a.as_bytes())
    });
    Some((host, port, auth_b64))
}

#[allow(clippy::double_ended_iterator_last)]
fn host_of(endpoint: &str) -> Option<String> {
    endpoint
        .split("://")
        .last()
        .and_then(|r| r.split('/').next())
        .and_then(|h| h.rsplit('@').next())
        .and_then(|h| h.split(':').next())
        .map(|s| s.to_string())
}

async fn request_via<C>(
    client: &PinnedClient<C>,
    endpoint: &str,
    bearer: &str,
    body: &serde_json::Value,
) -> std::result::Result<hyper::Response<hyper::body::Incoming>, String>
where
    C: tower_service::Service<Uri, Response = TokioIo<TcpStream>> + Clone + Send + Sync + 'static,
    C::Future: Send,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    // `stream` callers need SSE framing; non-stream want JSON. Send both so
    // pinned transport matches reqwest behavior per call site.
    let is_stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let req = Request::builder()
        .method("POST")
        .uri(endpoint)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"))
        .header(
            "accept",
            if is_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(Full::new(Bytes::from(body.to_string())))
        .map_err(|e| format!("pinned request build: {e}"))?;
    tokio::time::timeout(OPEN_TIMEOUT, client.request(req))
        .await
        .map_err(|_| "pinned open timeout (60s)".to_string())?
        .map_err(|e| format!("pinned transport: {e}"))
}

/// Non-streaming pinned POST: returns HTTP status + full body bytes.
pub async fn post_pinned(
    http: &reqwest::Client,
    endpoint: &str,
    bearer: &str,
    body: &serde_json::Value,
) -> std::result::Result<(u16, Bytes), String> {
    let host = host_of(endpoint).ok_or_else(|| "pinned: bad endpoint URL".to_string())?;
    let port = 443u16;
    let ips = doh_resolve(http, &host).await?;
    let tls = tls_config();
    let mut last_err = String::new();

    // Bound worst case: max 4 IPs x 30s each (previously 4+4 x 300s could
    // stall the runner ~40min). Per-IP timeout keeps VPN/DNS stalls bounded;
    // callers still have the outer 300s/600s reqwest caps.
    let ips: Vec<_> = ips.into_iter().take(4).collect();
    // 1. direct to each real IP.
    for ip in &ips {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls.clone())
            .https_only()
            .enable_http1()
            .wrap_connector(PinnedConnector { ip: *ip });
        let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(https);
        match tokio::time::timeout(PER_IP_TIMEOUT, request_via(&client, endpoint, bearer, body))
            .await
        {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let bytes = resp
                    .into_body()
                    .collect()
                    .await
                    .map_err(|e| format!("pinned body: {e}"))?
                    .to_bytes();
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    ip = %ip,
                    status = status,
                    "pinned direct succeeded"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    "llm pinned direct debug"
                );
                if status < 200 || status >= 300 {
                    tracing::warn!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        status,
                        "llm pinned direct non-2xx"
                    );
                }
                if status >= 400 {
                    tracing::error!(
                        target: deepseekcode_telemetry::LLM,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        status,
                        "llm pinned direct error status"
                    );
                }
                return Ok((status, bytes));
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    error_summary = %deepseekcode_telemetry::summarize_prompt(&e.to_string(), false),
                    "llm pinned direct attempt failed"
                );
                last_err = format!("pinned-direct[{ip}]: {e}");
            }
            Err(_) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    ip = %ip,
                    "llm pinned direct per-IP timeout"
                );
                last_err = format!("pinned-direct[{ip}]: per-IP timeout (30s)");
            }
        }
    }

    // 2. via system/env HTTP proxy (CONNECT to real IP).
    if let Some(proxy_url) = crate::http_client::resolve_proxy_url() {
        if let Some((ph, pp, auth)) = parse_proxy(&proxy_url) {
            for ip in &ips {
                let conn = ProxiedPinnedConnector {
                    target: ProxyTarget {
                        proxy_host: ph.clone(),
                        proxy_port: pp,
                        proxy_auth: auth.clone(),
                        real_ip: *ip,
                        real_port: port,
                        real_host: host.clone(),
                    },
                };
                let https = hyper_rustls::HttpsConnectorBuilder::new()
                    .with_tls_config(tls.clone())
                    .https_only()
                    .enable_http1()
                    .wrap_connector(conn);
                let client: Client<_, Full<Bytes>> =
                    Client::builder(TokioExecutor::new()).build(https);
                match tokio::time::timeout(
                    PER_IP_TIMEOUT,
                    request_via(&client, endpoint, bearer, body),
                )
                .await
                {
                    Ok(Ok(resp)) => {
                        let status = resp.status().as_u16();
                        let bytes = resp
                            .into_body()
                            .collect()
                            .await
                            .map_err(|e| format!("pinned-proxy body: {e}"))?
                            .to_bytes();
                        tracing::info!(
                            target: deepseekcode_telemetry::LLM,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                            ip = %ip,
                            status = status,
                            "pinned proxy succeeded"
                        );
                        tracing::debug!(
                            target: deepseekcode_telemetry::LLM,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                            "llm pinned proxy debug"
                        );
                        if status >= 400 {
                            tracing::error!(
                                target: deepseekcode_telemetry::LLM,
                                run_id = %deepseekcode_telemetry::current_run_id(),
                                status,
                                "llm pinned proxy error status"
                            );
                        }
                        return Ok((status, bytes));
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            target: deepseekcode_telemetry::LLM,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            error_summary = %deepseekcode_telemetry::summarize_prompt(&e.to_string(), false),
                            "llm pinned proxy attempt failed"
                        );
                        last_err = format!("{last_err} || pinned-proxy[{ip}]: {e}");
                    }
                    Err(_) => {
                        tracing::warn!(
                            target: deepseekcode_telemetry::LLM,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            ip = %ip,
                            "llm pinned proxy per-IP timeout"
                        );
                        last_err = format!("{last_err} || pinned-proxy[{ip}]: total timeout");
                    }
                }
            }
        }
    }

    Err(last_err)
}

/// Streaming pinned POST: returns SSE event stream on HTTP success.
/// On HTTP error status, returns `Err("HTTP <code>: <body>")` so callers can
/// apply the DeepSeek-only-fields 400 retry like the reqwest path.
pub async fn stream_pinned(
    http: &reqwest::Client,
    endpoint: &str,
    bearer: &str,
    body: &serde_json::Value,
) -> std::result::Result<Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>, String> {
    let host = host_of(endpoint).ok_or_else(|| "pinned: bad endpoint URL".to_string())?;
    let port = 443u16;
    let ips = doh_resolve(http, &host).await?;
    let tls = tls_config();
    let mut last_err = String::new();

    // Prefer direct (fewer moving parts), then proxy.
    let mut attempts: Vec<(String, ProxiedPinnedConnectorOrDirect)> = Vec::new();
    for ip in &ips {
        attempts.push((
            format!("direct[{ip}]"),
            ProxiedPinnedConnectorOrDirect::Direct(PinnedConnector { ip: *ip }),
        ));
    }
    if let Some(proxy_url) = crate::http_client::resolve_proxy_url() {
        if let Some((ph, pp, auth)) = parse_proxy(&proxy_url) {
            for ip in &ips {
                attempts.push((
                    format!("proxy[{ip}]"),
                    ProxiedPinnedConnectorOrDirect::Proxied(ProxiedPinnedConnector {
                        target: ProxyTarget {
                            proxy_host: ph.clone(),
                            proxy_port: pp,
                            proxy_auth: auth.clone(),
                            real_ip: *ip,
                            real_port: port,
                            real_host: host.clone(),
                        },
                    }),
                ));
            }
        }
    }

    for (label, conn) in attempts {
        let res: std::result::Result<hyper::Response<hyper::body::Incoming>, String> = match conn {
            ProxiedPinnedConnectorOrDirect::Direct(c) => {
                let https = hyper_rustls::HttpsConnectorBuilder::new()
                    .with_tls_config(tls.clone())
                    .https_only()
                    .enable_http1()
                    .wrap_connector(c);
                let client: Client<_, Full<Bytes>> =
                    Client::builder(TokioExecutor::new()).build(https);
                tokio::time::timeout(PER_IP_TIMEOUT, request_via(&client, endpoint, bearer, body))
                    .await
                    .map_err(|_| format!("{label}: per-IP timeout (30s)"))?
            }
            ProxiedPinnedConnectorOrDirect::Proxied(c) => {
                let https = hyper_rustls::HttpsConnectorBuilder::new()
                    .with_tls_config(tls.clone())
                    .https_only()
                    .enable_http1()
                    .wrap_connector(c);
                let client: Client<_, Full<Bytes>> =
                    Client::builder(TokioExecutor::new()).build(https);
                tokio::time::timeout(PER_IP_TIMEOUT, request_via(&client, endpoint, bearer, body))
                    .await
                    .map_err(|_| format!("{label}: per-IP timeout (30s)"))?
            }
        };
        match res {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if !resp.status().is_success() {
                    let bytes = resp
                        .into_body()
                        .collect()
                        .await
                        .map_err(|e| format!("{label} error body: {e}"))?
                        .to_bytes();
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    // HTTP error (e.g. 400 thinking-field rejection): surface
                    // with status so the caller can retry with a bare body.
                    // Only treat transport-level attempts as retryable here;
                    // an HTTP response means the network path works.
                    return Err(format!("HTTP {status}: {text}"));
                }
                tracing::info!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    endpoint_host = %crate::redact_endpoint_for_log(endpoint),
                    path = %label,
                    status = status,
                    "pinned stream opened"
                );
                tracing::debug!(
                    target: deepseekcode_telemetry::LLM,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    path = %label,
                    "llm pinned stream debug"
                );
                let stream = http_body_util::BodyDataStream::new(resp.into_body())
                    .map(|r| r.map_err(|e| DeepSeekCodeError::Llm(e.to_string())))
                    .scan(
                        (
                            Vec::<u8>::new(),
                            HashMap::<usize, (String, String, String)>::new(),
                        ),
                        |state, chunk| {
                            let (buffer, tool_acc) = state;
                            match chunk {
                                Ok(bytes) => {
                                    // Byte-level buffering + CRLF normalization,
                                    // mirroring the reqwest SSE paths.
                                    buffer.extend_from_slice(&bytes);
                                    if buffer.len() > 1024 * 1024 {
                                        buffer.clear();
                                        return futures::future::ready(Some(vec![]));
                                    }
                                    let text = String::from_utf8_lossy(buffer)
                                        .to_string()
                                        .replace("\r\n", "\n");
                                    let mut frames: Vec<String> = Vec::new();
                                    let mut remainder = text.as_str();
                                    while let Some(pos) = remainder.find("\n\n") {
                                        frames.push(remainder[..pos + 2].to_string());
                                        remainder = &remainder[pos + 2..];
                                    }
                                    let drain = (text.len() - remainder.len()).min(buffer.len());
                                    let _ = buffer.drain(..drain);
                                    let mut events = Vec::new();
                                    for frame in &frames {
                                        events.extend(crate::providers::openai::parse_sse_frame(
                                            frame, tool_acc,
                                        ));
                                    }
                                    futures::future::ready(Some(events))
                                }
                                Err(e) => futures::future::ready(Some(vec![Err(e)])),
                            }
                        },
                    )
                    .flat_map(futures::stream::iter);
                return Ok(Box::pin(stream));
            }
            Err(e) => {
                last_err = if last_err.is_empty() {
                    format!("{label}: {e}")
                } else {
                    format!("{last_err} || {label}: {e}")
                };
            }
        }
    }
    Err(last_err)
}

enum ProxiedPinnedConnectorOrDirect {
    Direct(PinnedConnector),
    Proxied(ProxiedPinnedConnector),
}

/// Parse a pinned non-streaming response body into `LlmResponse`.
pub fn parse_pinned_response(status: u16, bytes: &Bytes) -> Result<LlmResponse> {
    if !(200..300).contains(&status) {
        let text = String::from_utf8_lossy(bytes).to_string();
        return Err(DeepSeekCodeError::Llm(text));
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| DeepSeekCodeError::Llm(e.to_string()))?;
    crate::providers::openai::parse_message_response(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_ip_ranges_detected() {
        assert!(is_fake_ip(&"198.18.0.82".parse().unwrap()));
        assert!(is_fake_ip(&"198.19.5.6".parse().unwrap()));
        // Other TUN/CGNAT/private space must also be rejected for public hosts.
        assert!(is_fake_ip(&"10.0.0.5".parse().unwrap()));
        assert!(is_fake_ip(&"100.64.0.1".parse().unwrap()));
        assert!(is_fake_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_fake_ip(&"172.16.0.1".parse().unwrap()));
        assert!(!is_fake_ip(&"3.173.21.63".parse().unwrap()));
        assert!(!is_fake_ip(&"114.114.114.114".parse().unwrap()));
    }

    #[test]
    fn proxy_url_parsed() {
        let (h, p, a) = parse_proxy("http://127.0.0.1:10001").unwrap();
        assert_eq!((h.as_str(), p), ("127.0.0.1", 10001));
        assert!(a.is_none());
        let (h, p, _) = parse_proxy("127.0.0.1:7890").unwrap();
        assert_eq!((h.as_str(), p), ("127.0.0.1", 7890));
    }

    #[test]
    fn endpoint_host_extracted() {
        assert_eq!(
            host_of("https://api.deepseek.com/chat/completions").as_deref(),
            Some("api.deepseek.com")
        );
    }
}
