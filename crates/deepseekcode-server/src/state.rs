use deepseekcode_core::DeepSeekCodeEngine;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::pty::PtyManager;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<RwLock<DeepSeekCodeEngine>>,
    pub password: Option<String>,
    pub pty: Arc<PtyManager>,
}
