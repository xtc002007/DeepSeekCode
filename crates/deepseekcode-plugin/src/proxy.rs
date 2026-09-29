use std::sync::{Arc, RwLock};

type ToolProxyFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

static TOOL_PROXY: RwLock<Option<ToolProxyFn>> = RwLock::new(None);

/// Register a synchronous proxy used by WASM `tool_execute` host imports.
pub fn set_wasm_tool_proxy(proxy: ToolProxyFn) {
    if let Ok(mut guard) = TOOL_PROXY.write() {
        *guard = Some(proxy);
    } else {
        tracing::error!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "wasm tool proxy set failed: lock poisoned"
        );
    }
}

pub fn clear_wasm_tool_proxy() {
    if let Ok(mut guard) = TOOL_PROXY.write() {
        *guard = None;
    } else {
        tracing::error!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "wasm tool proxy clear failed: lock poisoned"
        );
    }
}

pub fn wasm_tool_execute(tool: &str, args: &str) -> Result<String, String> {
    // Never log `args`全文: it may carry tool secrets; log only tool name + run_id.
    let guard = TOOL_PROXY.read().map_err(|e| {
        tracing::error!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            tool = %tool,
            error = %e,
            "wasm tool proxy lock failed"
        );
        "wasm tool proxy lock poisoned".to_string()
    })?;
    let proxy = guard.as_ref().ok_or_else(|| {
        tracing::error!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            tool = %tool,
            "wasm tool proxy not configured"
        );
        "wasm tool proxy not configured".to_string()
    })?;
    proxy(tool, args).map_err(|e| {
        tracing::error!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            tool = %tool,
            error = %e,
            "wasm tool proxy call failed"
        );
        e
    })
}
