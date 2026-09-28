use axum::{
    body::Body,
    http::{HeaderValue, Request},
    middleware::Next,
    response::Response,
};
use ulid::Ulid;

pub const REQUEST_ID_HEADER: &str = "x-deepseekcode-request-id";

fn sanitize_request_id(raw: &str) -> Option<String> {
    // Client-supplied IDs are echoed + logged: cap length and charset to
    // block log injection / oversized-header DoS.
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub async fn request_id_middleware(mut req: Request<Body>, next: Next) -> Response {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_request_id)
        .unwrap_or_else(|| Ulid::new().to_string());

    let span = tracing::span!(
        target: deepseekcode_telemetry::SERVER,
        tracing::Level::INFO,
        "http_request",
        request_id = %request_id,
        method = %req.method(),
        path = %req.uri().path(),
    );
    let _guard = span.enter();

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert(REQUEST_ID_HEADER, value);
    }

    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }

    let status = response.status().as_u16();
    if status >= 500 {
        tracing::error!(
            target: deepseekcode_telemetry::SERVER,
            request_id = %request_id,
            status = status,
            "request failed"
        );
    } else {
        tracing::debug!(
            target: deepseekcode_telemetry::SERVER,
            request_id = %request_id,
            status = status,
            "request completed"
        );
    }

    response
}
