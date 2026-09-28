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
