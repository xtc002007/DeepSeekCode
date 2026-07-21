use async_trait::async_trait;
use deepseekcode_types::Result;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::CStr;
use std::path::Path;
use std::sync::Arc;
use wasmtime::*;

use crate::hooks::{
    ChatSystemTransformInput, ChatSystemTransformOutput, HookRegistry, SessionHook,
};
use crate::wasm::WasmPluginRuntime;

pub trait DeepSeekCodePlugin: Send + Sync {
    fn id(&self) -> &str;
    fn on_load(&self) -> Result<()> {
        Ok(())
    }
}

/// Built-in hook that prepends a marker to the system prompt (hello-hook demo).
struct NativeSystemEchoHook;

#[async_trait]
impl SessionHook for NativeSystemEchoHook {
    fn id(&self) -> &str {
        "hello-hook"
    }

    async fn chat_system_transform(
        &self,
        _input: &ChatSystemTransformInput,
        output: &mut ChatSystemTransformOutput,
    ) -> Result<()> {
        for line in &mut output.system {
            if !line.starts_with("[hello-hook]") {
                *line = format!("[hello-hook] {line}");
            }
        }
        Ok(())
    }
}

pub struct PluginHost {
    plugins: Vec<Box<dyn DeepSeekCodePlugin>>,
    wasm_modules: HashMap<String, Module>,
    native_hooks: Vec<Arc<dyn SessionHook>>,
    native_libraries: Vec<Library>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            wasm_modules: HashMap::new(),
            native_hooks: Vec::new(),
            native_libraries: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn DeepSeekCodePlugin>) {
        let _ = plugin.on_load();
        self.plugins.push(plugin);
    }

    pub fn load_wasm_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        let engine = WasmPluginRuntime::create_engine();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("wasm") {
                let path = entry.path();
                WasmPluginRuntime::validate(&path)?;
                let module = Module::from_file(&engine, &path)
                    .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
                self.wasm_modules.insert(
                    path.file_name().unwrap().to_string_lossy().to_string(),
                    module,
                );
            }
        }
        Ok(())
    }

    fn is_native_plugin(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("dll") | Some("so") | Some("dylib")
        )
    }

    pub fn load_native_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if Self::is_native_plugin(&path) {
                if let Err(e) = self.load_native_plugin(&path) {
                    tracing::warn!(
                        target: deepseekcode_telemetry::APP,
                        path = %path.display(),
                        error = %e,
                        "skipping native plugin"
                    );
                }
            }
        }
        Ok(())
    }

    /// Load a single native plugin library by path.
    pub fn load_native_file(&mut self, path: &Path) -> Result<()> {
        if Self::is_native_plugin(path) {
            self.load_native_plugin(path)
        } else {
            Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                "not a native plugin library: {}",
                path.display()
            )))
        }
    }

    fn load_native_plugin(&mut self, path: &Path) -> Result<()> {
        // SAFETY: plugin init must not unwind across the FFI boundary.
        unsafe {
            let library = Library::new(path).map_err(|e| {
                deepseekcode_types::DeepSeekCodeError::Other(format!(
                    "failed to load native plugin {}: {e}",
                    path.display()
                ))
            })?;

            let init: Symbol<unsafe extern "C" fn() -> i32> =
                library.get(b"deepseekcode_plugin_init").map_err(|e| {
                    deepseekcode_types::DeepSeekCodeError::Other(format!(
                        "native plugin {} missing deepseekcode_plugin_init: {e}",
                        path.display()
                    ))
                })?;
            if init() != 0 {
                return Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
                    "native plugin {} init failed",
                    path.display()
                )));
            }

            let id_fn: Symbol<unsafe extern "C" fn() -> *const u8> =
                library.get(b"deepseekcode_plugin_id").map_err(|e| {
                    deepseekcode_types::DeepSeekCodeError::Other(format!(
                        "native plugin {} missing deepseekcode_plugin_id: {e}",
                        path.display()
                    ))
                })?;
            let plugin_id = CStr::from_ptr(id_fn() as *const std::ffi::c_char)
                .to_string_lossy()
                .trim_end_matches('\0')
                .to_string();

            tracing::info!(
                target: deepseekcode_telemetry::APP,
                plugin = %plugin_id,
                path = %path.display(),
                "loaded native plugin"
            );

            if plugin_id == "hello-hook" {
                self.register_hello_hook();
            }

            self.native_libraries.push(library);
        }
        Ok(())
    }

    fn register_hello_hook(&mut self) {
        if self.native_hooks.iter().any(|h| h.id() == "hello-hook") {
            return;
        }
        self.native_hooks.push(Arc::new(NativeSystemEchoHook));
    }

    pub fn load_from_config(&mut self, plugins: &[String], project_dir: &str) -> Result<()> {
        let local_dir = Path::new(project_dir).join(".deepseekcode").join("plugins");
        self.load_wasm_dir(&local_dir)?;
        self.load_native_dir(&local_dir)?;

        for entry in plugins {
            let path = Path::new(entry);
            if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                WasmPluginRuntime::validate(path)?;
                let engine = WasmPluginRuntime::create_engine();
                let module = Module::from_file(&engine, path)
                    .map_err(|e| deepseekcode_types::DeepSeekCodeError::Other(e.to_string()))?;
                self.wasm_modules.insert(
                    path.file_name().unwrap().to_string_lossy().to_string(),
                    module,
                );
            } else if Self::is_native_plugin(path) {
                self.load_native_plugin(path)?;
            } else if entry == "hello-hook" || entry.ends_with("hello-hook") {
                tracing::info!(
                    target: deepseekcode_telemetry::APP,
                    plugin = %entry,
                    "registered built-in hello-hook"
                );
                self.register_hello_hook();
            } else {
                tracing::info!(
                    target: deepseekcode_telemetry::APP,
                    plugin = %entry,
                    "native plugin loading deferred; use .dll/.so in .deepseekcode/plugins/"
                );
            }
        }
        Ok(())
    }

    pub fn register_hooks(&self, registry: &mut HookRegistry) {
        for hook in &self.native_hooks {
            registry.register(hook.clone());
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.plugins
            .iter()
            .map(|p| p.id().to_string())
            .chain(self.wasm_modules.keys().cloned())
            .chain(self.native_hooks.iter().map(|h| h.id().to_string()))
            .collect()
    }

    pub fn invoke_wasm(&self, name: &str) -> Result<bool> {
        self.invoke_wasm_export(name, "_initialize")
            .map(|_| true)
            .or_else(|_| Ok(self.wasm_modules.contains_key(name)))
    }

    pub fn invoke_wasm_export(&self, name: &str, export: &str) -> Result<()> {
        let module = self.wasm_modules.get(name).ok_or_else(|| {
            deepseekcode_types::DeepSeekCodeError::NotFound(format!("wasm plugin {name}"))
        })?;
        WasmPluginRuntime::invoke(module, name, export)
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}
