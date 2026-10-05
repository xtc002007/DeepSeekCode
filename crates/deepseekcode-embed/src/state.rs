//! Shared Axum state. Pure data holder (`engine` + `password` + `pty`):
//! no business logic lives here, so this module emits no `tracing!` sites.
//! Product logging for HTTP boundary lives in `routes.rs` (SERVER target
//! with `run_id`), auth in `auth.rs`, PTY lifecycle in `pty.rs`.
use deepseekcode_core::DeepSeekCodeEngine;
use deepseekcode_i18n::LanguagePref;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::pty::PtyManager;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<RwLock<DeepSeekCodeEngine>>,
    pub password: Option<String>,
    pub pty: Arc<PtyManager>,
    /// Project preference used when a request does not say otherwise.
    /// `Auto` follows the process environment and OS language.
    pub locale: LanguagePref,
}
