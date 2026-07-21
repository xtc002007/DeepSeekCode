use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;

use crate::state::AppState;

pub async fn basic_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = &state.password else {
        return Ok(next.run(request).await);
    };

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let authorized = auth_header.is_some_and(|h| {
        h.strip_prefix("Basic ")
            .and_then(|encoded| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .ok()
            })
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .is_some_and(|creds| creds == format!("deepseekcode:{expected}"))
    });

    if authorized {
        Ok(next.run(request).await)
    } else {
        tracing::warn!(
            target: deepseekcode_telemetry::SERVER,
            path = %request.uri().path(),
            "basic auth failed"
        );
        Err(StatusCode::UNAUTHORIZED)
    }
}
