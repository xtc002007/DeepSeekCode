//! No-op WASM surface for `--no-default-features` builds.
//!
//! Mirrors the subset of `wasm::WasmPluginRuntime` that `host.rs` needs so
//! native-only binaries compile without wasmtime. Every entry point fails
//! closed with a descriptive error instead of silently pretending success.

use deepseekcode_types::{DeepSeekCodeError, Result};
use std::path::Path;

pub struct WasmPluginRuntime;

/// Stand-in for `wasmtime::Module` in no-wasm builds: constructible (so map
/// plumbing typechecks) but never produced by any loader path, so no WASM
/// code can execute. All entry points below fail closed regardless.
#[derive(Clone, Debug)]
pub struct StubModule {
    _private: (),
}

impl WasmPluginRuntime {
    pub fn validate(path: &Path) -> Result<()> {
        Err(DeepSeekCodeError::Other(format!(
            "wasm plugins are disabled in this build ({} not loaded)",
            path.display()
        )))
    }

    pub fn create_engine() -> StubEngine {
        StubEngine
    }
}

/// Placeholder for the shared wasmtime `Engine` in no-wasm builds.
#[derive(Clone, Debug)]
pub struct StubEngine;
