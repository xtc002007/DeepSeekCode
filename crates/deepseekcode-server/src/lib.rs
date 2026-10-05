//! Standalone-server facade.
//!
//! The engine implementation (Axum router, SSE, PTY, auth) lives in
//! `deepseekcode-embed` so SDK/desktop clients don't link a crate named
//! "server". This crate re-exports the full surface for the `serve` command
//! and existing `deepseekcode_server::` import paths.

pub use deepseekcode_embed::{app, auth, language, openapi, pty, request_id, routes, state};
pub use deepseekcode_embed::{build_router, serve, AppState, PtyManager};
