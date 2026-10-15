//! Standalone-server facade.
//!
//! The engine implementation (Axum router, SSE, PTY, auth) lives in
//! `code-plus-plus-embed` so SDK/desktop clients don't link a crate named
//! "server". This crate re-exports the full surface for the `serve` command
//! and existing `code_plus_plus_server::` import paths.

pub use code_plus_plus_embed::{app, auth, language, openapi, pty, request_id, routes, state};
pub use code_plus_plus_embed::{build_router, serve, AppState, PtyManager};
