//! In-process HTTP boundary (Axum router + SSE + PTY) shared by SDK/desktop/`serve`.
//! This crate root only re-exports; product `tracing!` sites live in submodules:
//! `routes.rs` (SERVER + `run_id` + `session_id`), `auth.rs` (auth warn),
//! `pty.rs` (PTY lifecycle + `latency_ms`), `app.rs`/`request_id.rs` (serve + HTTP).
pub mod app;
pub mod auth;
pub mod language;
pub mod openapi;
pub mod pty;
pub mod request_id;
pub mod routes;
pub mod state;

pub use app::{build_router, serve};
pub use pty::PtyManager;
pub use state::AppState;
