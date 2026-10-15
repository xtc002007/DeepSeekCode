//! Plugin host boundary (native + WASM hooks). This crate root only re-exports;
//! product `tracing!` sites live in `hooks.rs` (hook execution failure,
//! TOOL/APP + `run_id`), `proxy.rs` (proxy lock/call failure, TOOL + `run_id`),
//! `wasm.rs`/`wasm_stub.rs` (WASM call failure) and `host.rs`/`verify.rs`.
pub mod hooks;
pub mod host;
pub mod proxy;
// Trust types are feature-independent (sha256 pinning + minisign plumbing
// need no wasmtime); only module execution is gated.
pub mod verify;
#[cfg(feature = "wasm")]
pub mod wasm;
#[cfg(not(feature = "wasm"))]
mod wasm_stub;

pub use hooks::{
    ChatSystemTransformInput, ChatSystemTransformOutput, HookRegistry, SessionCompactingInput,
    SessionCompactingOutput, SessionHook, ToolExecuteAfterInput, ToolExecuteAfterOutput,
    ToolExecuteBeforeInput, ToolExecuteBeforeOutput,
};
pub use host::PluginHost;
pub use proxy::{clear_wasm_tool_proxy, set_wasm_tool_proxy, wasm_tool_execute};
#[cfg(feature = "wasm")]
pub use wasm::{WasmHostState, WasmPluginRuntime};
#[cfg(not(feature = "wasm"))]
pub use wasm_stub::WasmPluginRuntime;
