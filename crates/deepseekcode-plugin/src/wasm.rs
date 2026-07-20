use deepseekcode_types::Result;
use std::cell::RefCell;
use std::path::Path;
use wasmtime::*;

pub struct WasmPluginRuntime;

/// Host state passed to WASM guest instances.
pub struct WasmHostState {
    pub plugin_id: String,
    pub transformed_config: RefCell<Option<String>>,
}

fn read_guest_bytes(caller: &mut Caller<'_, WasmHostState>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return None;
    }
    let memory = caller.get_export("memory")?.into_memory()?;
    let data = memory.data(caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    data.get(start..end).map(|slice| slice.to_vec())
}

fn read_guest_utf8(caller: &mut Caller<'_, WasmHostState>, ptr: i32, len: i32) -> Option<String> {
    let bytes = read_guest_bytes(caller, ptr, len)?;
    std::str::from_utf8(&bytes).ok().map(str::to_string)
}

impl WasmPluginRuntime {
    pub fn validate(path: &Path) -> Result<()> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, path)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let _ = module;
        Ok(())
    }

    /// Register host functions exposed to WASM plugins.
    pub fn link_host_functions(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "deepseekcode",
                "tool_execute",
                |mut caller: Caller<'_, WasmHostState>,
                 tool_ptr: i32,
                 tool_len: i32,
                 args_ptr: i32,
                 args_len: i32|
                 -> i32 {
                    let plugin_id = caller.data().plugin_id.clone();
                    let tool = read_guest_utf8(&mut caller, tool_ptr, tool_len).unwrap_or_default();
                    let args = read_guest_utf8(&mut caller, args_ptr, args_len).unwrap_or_default();
                    match crate::proxy::wasm_tool_execute(&tool, &args) {
                        Ok(result) => {
                            tracing::debug!(
                                target: deepseekcode_telemetry::APP,
                                plugin_id = %plugin_id,
                                tool = %tool,
                                result_len = result.len(),
                                "wasm host: tool_execute proxied"
                            );
                            0
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: deepseekcode_telemetry::APP,
                                plugin_id = %plugin_id,
                                tool = %tool,
                                error = %e,
                                "wasm host: tool_execute failed"
                            );
                            -1
                        }
                    }
                },
            )
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        linker
            .func_wrap(
                "deepseekcode",
                "config_transform",
                |mut caller: Caller<'_, WasmHostState>, config_ptr: i32, config_len: i32| -> i32 {
                    let plugin_id = caller.data().plugin_id.clone();
                    let config =
                        read_guest_utf8(&mut caller, config_ptr, config_len).unwrap_or_default();
                    let transformed = match serde_json::from_str::<serde_json::Value>(&config) {
                        Ok(mut value) => {
                            if let Some(obj) = value.as_object_mut() {
                                obj.insert(
                                    "_wasm_transformed_by".to_string(),
                                    serde_json::json!(plugin_id),
                                );
                            }
                            serde_json::to_string(&value).unwrap_or(config)
                        }
                        Err(_) => config,
                    };
                    *caller.data().transformed_config.borrow_mut() = Some(transformed);
                    tracing::debug!(
                        target: deepseekcode_telemetry::APP,
                        plugin_id = %plugin_id,
                        "wasm host: config_transform applied"
                    );
                    0
                },
            )
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        linker
            .func_wrap(
                "deepseekcode",
                "log",
                |mut caller: Caller<'_, WasmHostState>, ptr: i32, len: i32| {
                    let plugin_id = caller.data().plugin_id.clone();
                    let message = read_guest_utf8(&mut caller, ptr, len).unwrap_or_default();
                    tracing::info!(
                        target: deepseekcode_telemetry::APP,
                        plugin_id = %plugin_id,
                        message = %message,
                        "wasm plugin log"
                    );
                },
            )
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        Ok(())
    }

    pub fn create_engine() -> Engine {
        Engine::default()
    }

    /// Instantiate a WASM module and invoke an export (defaults to `_initialize`).
    pub fn invoke(module: &Module, plugin_id: &str, export: &str) -> Result<()> {
        let engine = Self::create_engine();
        let mut linker = Linker::new(&engine);
        Self::link_host_functions(&mut linker)?;
        let mut store = Store::new(
            &engine,
            WasmHostState {
                plugin_id: plugin_id.to_string(),
                transformed_config: RefCell::new(None),
            },
        );
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let func = instance
            .get_func(&mut store, export)
            .ok_or_else(|| {
                deepseekcode_types::DeepSeekCodeError::Other(format!("wasm export not found: {export}"))
            })?;
        func.call(&mut store, &[], &mut [])
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        Ok(())
    }
}
