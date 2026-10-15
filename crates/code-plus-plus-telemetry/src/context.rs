use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

static RUN_CONTEXT: OnceLock<RunContext> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunContext {
    pub run_id: String,
    pub component: String,
    pub app_version: String,
    pub pid: u32,
    pub platform: String,
    pub arch: String,
    pub run_dir: PathBuf,
    pub logs_root: PathBuf,
}

impl RunContext {
    pub fn global() -> Option<&'static RunContext> {
        RUN_CONTEXT.get()
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn set_global(ctx: RunContext) {
        let _ = RUN_CONTEXT.set(ctx);
    }
}

pub fn run_context() -> Option<&'static RunContext> {
    RunContext::global()
}

pub fn run_id() -> Option<String> {
    RunContext::global().map(|c| c.run_id.clone())
}
