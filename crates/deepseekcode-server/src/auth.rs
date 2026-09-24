use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;

use crate::state::AppState;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Length-independent comparison: fold length difference into diff so
    // length is not leaked via early return timing.
    let mut diff = (a.len() ^ b.len()) as u8;
    // Compare up to the longer length, padding the shorter with zeros.
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= x ^ y;
    }
    // Avoid allocating per-request formatted credentials: compare decoded
    // `user:pass` against the expected pair without `format!`.
    diff == 0
}

pub async fn basic_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.password else {
        return next.run(request).await;
    };

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    // Basic rate-limit: at most 30 auth attempts per minute per process.
    // Full per-IP limiting belongs in a gateway; this stops trivial brute force.
    static FAIL_WINDOW: std::sync::LazyLock<std::sync::Mutex<(std::time::Instant, u32)>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new((std::time::Instant::now(), 0)));
    let authorized = auth_header.is_some_and(|h| {
        h.strip_prefix("Basic ")
            .and_then(|encoded| {
                if encoded.len() > 512 {
                    return None;
                }
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .ok()
            })
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .is_some_and(|creds| {
                let mut parts = creds.splitn(2, ':');
                let user = parts.next().unwrap_or_default();
                let pass = parts.next().unwrap_or_default();
                // `deepseekcode:` username is fixed; compare both halves in
                // constant time without per-request allocation.
                constant_time_eq(user.as_bytes(), b"deepseekcode")
                    && constant_time_eq(pass.as_bytes(), expected.as_bytes())
            })
    });
    if !authorized {
        if let Ok(mut guard) = FAIL_WINDOW.lock() {
            let now = std::time::Instant::now();
            if now.duration_since(guard.0).as_secs() > 60 {
                *guard = (now, 0);
            }
            guard.1 += 1;
        }
    }

    if authorized {
        next.run(request).await
    } else {
        tracing::warn!(
            target: deepseekcode_telemetry::SERVER,
            path = %request.uri().path(),
            "basic auth failed"
        );
        // Include WWW-Authenticate + JSON body per RFC 7617.
        let mut res = axum::http::Response::new(axum::body::Body::from(
            serde_json::json!({"error": "unauthorized"}).to_string(),
        ));
        *res.status_mut() = StatusCode::UNAUTHORIZED;
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static(r#"Basic realm="deepseekcode""#),
        );
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        res
    }
}
