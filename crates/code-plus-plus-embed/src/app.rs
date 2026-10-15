use axum::http::Request;
use axum::middleware;
use axum::Router;
use code_plus_plus_core::CodePlusPlusEngine;
use code_plus_plus_types::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::auth::basic_auth_middleware;
use crate::language::locale_middleware;
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
    // Standalone `serve` may run without CLI/Desktop init (library use).
    let run_ctx = code_plus_plus_telemetry::ensure_init("server", Some(project_dir.as_str()));
    let run_id = run_ctx.run_id.clone();
    let engine = match CodePlusPlusEngine::open(project_dir.clone(), &db_path).await {
        Ok(e) => Arc::new(RwLock::new(e)),
        Err(e) => {
            tracing::error!(
                target: code_plus_plus_telemetry::SERVER,
                run_id = %run_id,
                project_dir = %project_dir,
                db_path = %db_path,
                error = %e,
                "server engine open failed"
            );
            return Err(e);
        }
    };
    let state = AppState {
        engine,
        password,
        pty: Arc::new(PtyManager::new()),
        locale: code_plus_plus_config::load_effective_config(camino::Utf8Path::new(&project_dir))
            .map(|config| code_plus_plus_config::resolve_language(&config))
            .unwrap_or_default(),
    };
    let app = build_router(state);

    info!(
        target: code_plus_plus_telemetry::SERVER,
        run_id = %run_id,
        %addr,
        project_dir = %project_dir,
        "Code++ server listening"
    );
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                target: code_plus_plus_telemetry::SERVER,
                run_id = %run_id,
                %addr,
                error = %e,
                "server bind failed"
            );
            return Err(code_plus_plus_types::CodePlusPlusError::Other(e.to_string()));
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(
            target: code_plus_plus_telemetry::SERVER,
            run_id = %run_id,
            %addr,
            error = %e,
            "server exited with error"
        );
        return Err(code_plus_plus_types::CodePlusPlusError::Other(e.to_string()));
    }
    tracing::info!(
        target: code_plus_plus_telemetry::SERVER,
        run_id = %run_id,
        %addr,
        "server shutdown cleanly"
    );
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    // request_id must be the OUTERMOST layer so even 401s carry request-id.
    // Axum `.layer()` wraps: last call = outermost. So auth first, then
    // request_id last to sit outside auth.
    // `/health` and `/api/v1/openapi.json` stay OUTSIDE auth so load
    // balancers / probes work when a password is set.
    let authed = routes::authed_router(state.clone()).layer(middleware::from_fn_with_state(
        state.clone(),
        basic_auth_middleware,
    ));
    let public = Router::new()
        .route("/health", axum::routing::get(routes::health_root))
        .route("/api/v1/health", axum::routing::get(routes::health))
        .route(
            "/api/v1/openapi.json",
            axum::routing::get(routes::openapi_json),
        );
    public
        .merge(authed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            locale_middleware,
        ))
        .layer(middleware::from_fn(request_id_middleware))
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                tracing::span!(
                    target: code_plus_plus_telemetry::SERVER,
                    tracing::Level::INFO,
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                )
            }),
        )
        .layer(cors_layer())
}

fn cors_layer() -> CorsLayer {
    // Default deny: loopback only. `permissive()` (Any origin) would let any
    // website call a localhost server with stored Basic credentials.
    let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"];
    match std::env::var("CODE_PLUS_PLUS_CORS_ORIGINS") {
        Ok(raw) if !raw.trim().is_empty() => {
            let origins: Vec<axum::http::HeaderValue> = raw
                .split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect();
            if origins.is_empty() {
                CorsLayer::new()
                    .allow_origin(
                        loopback
                            .iter()
                            .filter_map(|o| o.parse().ok())
                            .collect::<Vec<axum::http::HeaderValue>>(),
                    )
                    .allow_methods(Any)
                    .allow_headers(Any)
            } else {
                CorsLayer::new()
                    .allow_origin(origins)
                    .allow_methods(Any)
                    .allow_headers(Any)
            }
        }
        _ => CorsLayer::new()
            .allow_origin(
                loopback
                    .iter()
                    .filter_map(|o| o.parse().ok())
                    .collect::<Vec<axum::http::HeaderValue>>(),
            )
            .allow_methods(Any)
            .allow_headers(Any),
    }
}
