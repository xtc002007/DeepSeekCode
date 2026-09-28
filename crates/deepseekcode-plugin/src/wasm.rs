use deepseekcode_types::Result;
use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;
use wasmtime::{Caller, Config, Engine, Instance, Linker, Module, ResourceLimiter, Store, Val};

pub struct WasmPluginRuntime;

/// Fuel per guest invocation: bounds CPU for untrusted plugin code.
/// Exhaustion surfaces as a trap (propagated as a hook error, never fatal).
pub const WASM_FUEL_PER_CALL: u64 = 10_000_000;
/// Linear-memory ceiling per guest instance (64 MiB).
pub const WASM_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Table/function cap per guest instance.
pub const WASM_MAX_TABLE_ELEMENTS: usize = 10_000;

/// Process-wide shared engine: compilation artifacts are cached across
/// plugin loads instead of rebuilding an `Engine` per call (previously every
/// `validate`/`invoke` paid full engine construction).
fn shared_engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.consume_fuel(true);
        Engine::new(&config).expect("wasmtime engine with fuel metering")
    })
}

/// Per-instance memory/table limiter: guest `memory.grow` beyond the ceiling
/// fails the grow (returns -1) instead of ballooning the host.
struct PluginLimiter;

impl ResourceLimiter for PluginLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= WASM_MAX_MEMORY_BYTES)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= WASM_MAX_TABLE_ELEMENTS)
    }
}

/// Build a fuel-metered, memory-limited store for one guest invocation.
fn limited_store(state: WasmHostState) -> Result<Store<WasmHostState>> {
    let engine = shared_engine();
    let mut store = Store::new(engine, state);
    store.set_fuel(WASM_FUEL_PER_CALL).map_err(|e| {
        deepseekcode_types::DeepSeekCodeError::Other(format!("wasm fuel setup: {e}"))
    })?;
    store.limiter(|state| &mut state.limiter);
    Ok(store)
}

/// Host state passed to WASM guest instances.
pub struct WasmHostState {
    pub plugin_id: String,
    pub transformed_config: RefCell<Option<String>>,
    pub transform_output: RefCell<Option<String>>,
    limiter: PluginLimiter,
}

impl WasmHostState {
    fn new(plugin_id: String) -> Self {
        Self {
            plugin_id,
            transformed_config: RefCell::new(None),
            transform_output: RefCell::new(None),
            limiter: PluginLimiter,
        }
    }

    fn with_config(plugin_id: String, config: Option<String>) -> Self {
        Self {
            plugin_id,
            transformed_config: RefCell::new(config),
            transform_output: RefCell::new(None),
            limiter: PluginLimiter,
        }
    }
}

pub(crate) fn read_guest_bytes(
    caller: &mut Caller<'_, WasmHostState>,
    ptr: i32,
    len: i32,
) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return None;
    }
    let memory = caller.get_export("memory")?.into_memory()?;
    let data = memory.data(caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    data.get(start..end).map(|slice| slice.to_vec())
}

fn read_guest_i32(caller: &mut Caller<'_, WasmHostState>, ptr: i32) -> Option<i32> {
    let bytes = read_guest_bytes(caller, ptr, 4)?;
    let arr: [u8; 4] = bytes.try_into().ok()?;
    Some(i32::from_le_bytes(arr))
}

fn write_guest_bytes(caller: &mut Caller<'_, WasmHostState>, ptr: i32, data: &[u8]) -> bool {
    if ptr < 0 {
        return false;
    }
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(memory) => memory,
        None => return false,
    };
    let start = ptr as usize;
    let end = match start.checked_add(data.len()) {
        Some(end) => end,
        None => return false,
    };
    let slice = match memory.data_mut(caller).get_mut(start..end) {
        Some(slice) => slice,
        None => return false,
    };
    slice.copy_from_slice(data);
    true
}

fn write_guest_i32(caller: &mut Caller<'_, WasmHostState>, ptr: i32, value: i32) -> bool {
    write_guest_bytes(caller, ptr, &value.to_le_bytes())
}

fn read_guest_utf8(caller: &mut Caller<'_, WasmHostState>, ptr: i32, len: i32) -> Option<String> {
    let bytes = read_guest_bytes(caller, ptr, len)?;
    std::str::from_utf8(&bytes).ok().map(str::to_string)
}

fn call_guest_alloc(
    instance: &Instance,
    store: &mut Store<WasmHostState>,
    size: i32,
) -> Result<i32> {
    let alloc = instance
        .get_func(&mut *store, "hook_alloc")
        .ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::Other("wasm export not found: hook_alloc".into())
        })?;
    let mut results = vec![Val::I32(0)];
    alloc
        .call(&mut *store, &[Val::I32(size)], &mut results)
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
    results
        .first()
        .and_then(|v| v.i32())
        .ok_or_else(|| deepseekcode_types::DeepSeekCodeError::Other("hook_alloc failed".into()))
}

fn write_guest_bytes_store(
    instance: &Instance,
    store: &mut Store<WasmHostState>,
    ptr: i32,
    data: &[u8],
) -> bool {
    if ptr < 0 {
        return false;
    }
    let memory = match instance
        .get_export(&mut *store, "memory")
        .and_then(|e| e.into_memory())
    {
        Some(memory) => memory,
        None => return false,
    };
    let start = ptr as usize;
    let end = match start.checked_add(data.len()) {
        Some(end) => end,
        None => return false,
    };
    let slice = match memory.data_mut(&mut *store).get_mut(start..end) {
        Some(slice) => slice,
        None => return false,
    };
    slice.copy_from_slice(data);
    true
}

fn read_guest_cstring_store(
    instance: &Instance,
    store: &mut Store<WasmHostState>,
    ptr: i32,
    max_len: i32,
) -> Option<String> {
    if ptr < 0 || max_len <= 0 {
        return None;
    }
    let memory = instance.get_export(&mut *store, "memory")?.into_memory()?;
    let data = memory.data(&mut *store);
    let start = ptr as usize;
    let end = start.checked_add(max_len as usize)?;
    let slice = data.get(start..end)?;
    let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    std::str::from_utf8(&slice[..nul]).ok().map(str::to_string)
}

enum BufferHookCall {
    Success { out_ptr: i32, out_max: i32 },
    BufferTooSmall,
    Failed(i32),
}

fn invoke_buffer_hook_inner(
    instance: &Instance,
    store: &mut Store<WasmHostState>,
    export: &str,
    input: &str,
    out_max: i32,
) -> Result<BufferHookCall> {
    let hook = instance.get_func(&mut *store, export).ok_or_else(|| {
        deepseekcode_types::DeepSeekCodeError::Other(format!("wasm export not found: {export}"))
    })?;
    let input_bytes = input.as_bytes();
    let in_ptr = call_guest_alloc(instance, store, input_bytes.len() as i32)?;
    if !write_guest_bytes_store(instance, store, in_ptr, input_bytes) {
        return Err(deepseekcode_types::DeepSeekCodeError::Other(
            "failed to write wasm hook input".into(),
        ));
    }
    let out_ptr = call_guest_alloc(instance, store, out_max)?;
    let mut results = vec![Val::I32(0)];
    hook.call(
        &mut *store,
        &[
            Val::I32(in_ptr),
            Val::I32(input_bytes.len() as i32),
            Val::I32(out_ptr),
            Val::I32(out_max),
        ],
        &mut results,
    )
    .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
    let status = results.first().and_then(|v| v.i32()).ok_or_else(|| {
        deepseekcode_types::DeepSeekCodeError::Other("wasm hook returned no status".into())
    })?;
    match status {
        0 => Ok(BufferHookCall::Success { out_ptr, out_max }),
        -2 => Ok(BufferHookCall::BufferTooSmall),
        other => Ok(BufferHookCall::Failed(other)),
    }
}

impl WasmPluginRuntime {
    pub fn validate(path: &Path) -> Result<()> {
        let module = Module::from_file(shared_engine(), path)
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
                 args_len: i32,
                 out_ptr: i32,
                 out_len_ptr: i32|
                 -> i32 {
                    let plugin_id = caller.data().plugin_id.clone();
                    let tool = read_guest_utf8(&mut caller, tool_ptr, tool_len).unwrap_or_default();
                    let args = read_guest_utf8(&mut caller, args_ptr, args_len).unwrap_or_default();
                    match crate::proxy::wasm_tool_execute(&tool, &args) {
                        Ok(result) => {
                            if out_ptr > 0 && out_len_ptr > 0 {
                                let capacity =
                                    read_guest_i32(&mut caller, out_len_ptr).unwrap_or(0);
                                if capacity < result.len() as i32 {
                                    tracing::warn!(
                                        target: deepseekcode_telemetry::APP,
                                        plugin_id = %plugin_id,
                                        tool = %tool,
                                        capacity,
                                        needed = result.len(),
                                        "wasm host: tool_execute output buffer too small"
                                    );
                                    return -2;
                                }
                                if !write_guest_bytes(&mut caller, out_ptr, result.as_bytes()) {
                                    return -1;
                                }
                                if !write_guest_i32(&mut caller, out_len_ptr, result.len() as i32) {
                                    return -1;
                                }
                            }
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

        linker
            .func_wrap(
                "deepseekcode",
                "system_transform_output",
                |mut caller: Caller<'_, WasmHostState>, ptr: i32, len: i32| -> i32 {
                    let text = read_guest_utf8(&mut caller, ptr, len).unwrap_or_default();
                    *caller.data().transform_output.borrow_mut() = Some(text);
                    0
                },
            )
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        linker
            .func_wrap(
                "deepseekcode",
                "system_transform_input",
                |mut caller: Caller<'_, WasmHostState>, ptr: i32, len_ptr: i32| -> i32 {
                    let Some(input) = caller.data().transformed_config.borrow().clone() else {
                        return -1;
                    };
                    let capacity = read_guest_i32(&mut caller, len_ptr).unwrap_or(0);
                    if capacity < input.len() as i32 {
                        return -2;
                    }
                    if !write_guest_bytes(&mut caller, ptr, input.as_bytes()) {
                        return -1;
                    }
                    if !write_guest_i32(&mut caller, len_ptr, input.len() as i32) {
                        return -1;
                    }
                    0
                },
            )
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;

        Ok(())
    }

    /// Shared, fuel-metered engine handle (cheap clone). Prefer passing
    /// `shared_engine()` directly; kept for callers constructing modules.
    pub fn create_engine() -> Engine {
        shared_engine().clone()
    }

    /// Instantiate a WASM module and invoke an export (defaults to `_initialize`).
    pub fn invoke(module: &Module, plugin_id: &str, export: &str) -> Result<()> {
        let engine = shared_engine();
        let mut linker = Linker::new(engine);
        Self::link_host_functions(&mut linker)?;
        let mut store = limited_store(WasmHostState::new(plugin_id.to_string()))?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let func = instance.get_func(&mut store, export).ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::Other(format!("wasm export not found: {export}"))
        })?;
        func.call(&mut store, &[], &mut [])
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        Ok(())
    }

    pub fn transform_system_prompt(
        module: &Module,
        plugin_id: &str,
        system: &[String],
        model: &str,
    ) -> Result<Option<Vec<String>>> {
        let input = serde_json::json!({ "system": system, "model": model }).to_string();
        if let Some(output) =
            Self::invoke_buffer_hook(module, plugin_id, "chat_system_transform", &input)?
        {
            return Self::parse_system_transform_output(&output);
        }

        let engine = shared_engine();
        let mut linker = Linker::new(engine);
        Self::link_host_functions(&mut linker)?;
        let input = serde_json::json!({ "system": system, "model": model }).to_string();
        let mut store = limited_store(WasmHostState::with_config(
            plugin_id.to_string(),
            Some(input),
        ))?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let Some(func) = instance.get_func(&mut store, "chat_system_transform") else {
            return Ok(None);
        };
        func.call(&mut store, &[], &mut [])
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let Some(output) = store.data().transform_output.borrow().clone() else {
            return Ok(None);
        };
        Self::parse_system_transform_output(&output)
    }

    fn parse_system_transform_output(output: &str) -> Result<Option<Vec<String>>> {
        let value: serde_json::Value = serde_json::from_str(output).unwrap_or(serde_json::json!({
            "system": output
        }));
        if let Some(lines) = value.get("system").and_then(|v| v.as_array()) {
            return Ok(Some(
                lines
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            ));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(vec![text.to_string()]));
        }
        Ok(None)
    }

    fn invoke_buffer_hook(
        module: &Module,
        plugin_id: &str,
        export: &str,
        input: &str,
    ) -> Result<Option<String>> {
        let engine = shared_engine();
        let mut linker = Linker::new(engine);
        Self::link_host_functions(&mut linker)?;
        let mut store = limited_store(WasmHostState::new(plugin_id.to_string()))?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        if instance.get_func(&mut store, "hook_alloc").is_none()
            || instance.get_func(&mut store, export).is_none()
        {
            return Ok(None);
        }

        let mut out_max = 64 * 1024;
        for _ in 0..4 {
            match invoke_buffer_hook_inner(&instance, &mut store, export, input, out_max)? {
                BufferHookCall::Success { out_ptr, out_max } => {
                    let text = read_guest_cstring_store(&instance, &mut store, out_ptr, out_max)
                        .ok_or_else(|| {
                            deepseekcode_types::DeepSeekCodeError::Other(
                                "wasm hook output unreadable".into(),
                            )
                        })?;
                    return Ok(Some(text));
                }
                BufferHookCall::BufferTooSmall => {
                    out_max = out_max.saturating_mul(2);
                }
                BufferHookCall::Failed(status) => {
                    return Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                        "wasm hook {export} failed with status {status}"
                    )));
                }
            }
        }
        Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
            "wasm hook {export} output buffer too small"
        )))
    }

    /// Invoke a WASM export using the buffer ABI (`hook_alloc` + `(in_ptr, in_len, out_ptr, out_max) -> i32`).
    pub fn invoke_json_hook(
        module: &Module,
        plugin_id: &str,
        export: &str,
        input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        let input_str = input.to_string();
        if let Some(output) = Self::invoke_buffer_hook(module, plugin_id, export, &input_str)? {
            let value =
                serde_json::from_str(&output).unwrap_or(serde_json::json!({ "raw": output }));
            return Ok(Some(value));
        }

        let engine = shared_engine();
        let mut linker = Linker::new(engine);
        Self::link_host_functions(&mut linker)?;
        let input_str = input.to_string();
        let mut store = limited_store(WasmHostState::with_config(
            plugin_id.to_string(),
            Some(input_str),
        ))?;
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let Some(func) = instance.get_func(&mut store, export) else {
            return Ok(None);
        };
        func.call(&mut store, &[], &mut [])
            .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
        let Some(output) = store.data().transform_output.borrow().clone() else {
            return Ok(None);
        };
        let value = serde_json::from_str(&output).unwrap_or(serde_json::json!({ "raw": output }));
        Ok(Some(value))
    }
}
