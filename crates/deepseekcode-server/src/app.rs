use axum::http::Request;
use axum::middleware;
use axum::Router;
use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_types::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::auth::basic_auth_middleware;
use crate::pty::PtyManager;
use crate::request_id::request_id_middleware;
use crate::routes;
use crate::state::AppState;

pub async fn serve(
    project_dir: String,
    db_path: String,
    addr: SocketAddr,
    password: Option<String>,
) -> Result<()> {
    let engine = Arc::new(RwLock::new(
        DeepSeekCodeEngine::open(project_dir, &db_path).await?,
    ));
    let state = AppState {
        engine,
        password,
        pty: Arc::new(PtyManager::new()),
    };
    let app = build_router(state);

    info!(
        target: deepseekcode_telemetry::SERVER,
        %addr,
        "DeepSeekCode server listening"
    );
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
    axum::serve(listener, app)
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    routes::router(state.clone())
        .layer(middleware::from_fn(request_id_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), basic_auth_middleware))
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                tracing::span!(
                    target: deepseekcode_telemetry::SERVER,
                    tracing::Level::INFO,
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                )
            }),
        )
        .layer(cors_layer())
}

fn cors_layer() -> CorsLayer {
    match std::env::var("DEEPSEEKCODE_CORS_ORIGINS") {
        Ok(raw) if !raw.trim().is_empty() => {
            let origins: Vec<axum::http::HeaderValue> = raw
                .split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect();
            if origins.is_empty() {
                CorsLayer::permissive()
            } else {
                CorsLayer::new()
                    .allow_origin(origins)
                    .allow_methods(Any)
                    .allow_headers(Any)
            }
        }
        _ => CorsLayer::permissive(),
    }
}
