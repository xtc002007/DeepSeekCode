use std::sync::{Arc, RwLock};

type ToolProxyFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

static TOOL_PROXY: RwLock<Option<ToolProxyFn>> = RwLock::new(None);

/// Register a synchronous proxy used by WASM `tool_execute` host imports.
pub fn set_wasm_tool_proxy(proxy: ToolProxyFn) {
    if let Ok(mut guard) = TOOL_PROXY.write() {
        *guard = Some(proxy);
    }
}

pub fn clear_wasm_tool_proxy() {
    if let Ok(mut guard) = TOOL_PROXY.write() {
        *guard = None;
    }
}

pub fn wasm_tool_execute(tool: &str, args: &str) -> Result<String, String> {
    let guard = TOOL_PROXY
        .read()
        .map_err(|_| "wasm tool proxy lock poisoned".to_string())?;
    let proxy = guard
        .as_ref()
        .ok_or_else(|| "wasm tool proxy not configured".to_string())?;
    proxy(tool, args)
}
