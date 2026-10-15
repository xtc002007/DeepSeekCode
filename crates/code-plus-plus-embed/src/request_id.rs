use axum::{
    body::Body,
    http::{HeaderValue, Request},
    middleware::Next,
    response::Response,
};
use ulid::Ulid;

pub const REQUEST_ID_HEADER: &str = "x-code-plus-plus-request-id";

// Task-local id lives in telemetry so `core` background tasks can join without
// depending on `embed` (which would cycle). Re-exported for compatibility.
pub use code_plus_plus_telemetry::{current_request_id, scope_request_id};

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
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let span = tracing::span!(
        target: code_plus_plus_telemetry::SERVER,
        tracing::Level::INFO,
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
        run_id = %code_plus_plus_telemetry::current_run_id(),
    );
    let _guard = span.enter();
    let start = std::time::Instant::now();

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert(REQUEST_ID_HEADER, value);
    }

    // Scope the id for the entire handler subtree so `service_error_*`
    // can attach it to the response extension for the single log below.
    let mut response = scope_request_id(request_id.clone(), next.run(req)).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }

    let status = response.status().as_u16();
    let duration_ms = start.elapsed().as_millis() as u64;
    // Single canonical log per request: failure detail (code + server-side
    // error + session) rides the response extension from `service_error_*`;
    // this layer adds what only it knows (latency + method + path).
    if let Some(detail) = response.extensions().get::<crate::routes::ApiErrorDetail>() {
        if status >= 500 {
            tracing::error!(
                target: code_plus_plus_telemetry::SERVER,
                request_id = %request_id,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                method = %method,
                path = %path,
                status = status,
                duration_ms = duration_ms,
                session_id = %detail.session_id,
                code = detail.code,
                error = %detail.error,
                "api request failed"
            );
        } else if status == 404 || status == 409 {
            tracing::warn!(
                target: code_plus_plus_telemetry::SERVER,
                request_id = %request_id,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                method = %method,
                path = %path,
                status = status,
                duration_ms = duration_ms,
                session_id = %detail.session_id,
                code = detail.code,
                error = %detail.error,
                "api request rejected"
            );
        } else {
            tracing::debug!(
                target: code_plus_plus_telemetry::SERVER,
                request_id = %request_id,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                method = %method,
                path = %path,
                status = status,
                duration_ms = duration_ms,
                session_id = %detail.session_id,
                code = detail.code,
                error = %detail.error,
                "api request client error"
            );
        }
    } else if status >= 500 {
        tracing::error!(
            target: code_plus_plus_telemetry::SERVER,
            request_id = %request_id,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration_ms,
            "request failed"
        );
    } else if status >= 400 {
        tracing::warn!(
            target: code_plus_plus_telemetry::SERVER,
            request_id = %request_id,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration_ms,
            "request client error"
        );
    } else {
        tracing::info!(
            target: code_plus_plus_telemetry::SERVER,
            request_id = %request_id,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration_ms,
            "request completed"
        );
    }

    response
}
