#[cfg(not(feature = "wasm"))]
use crate::wasm_stub::StubModule as Module;
use async_trait::async_trait;
use code_plus_plus_types::Result;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "wasm")]
use wasmtime::Module;

#[cfg(not(feature = "wasm"))]
use crate::hooks::SessionHook;
use crate::hooks::{ChatSystemTransformInput, ChatSystemTransformOutput, HookRegistry};
#[cfg(feature = "wasm")]
use crate::hooks::{
    SessionCompactingInput, SessionCompactingOutput, SessionHook, ToolExecuteAfterInput,
    ToolExecuteAfterOutput, ToolExecuteBeforeInput, ToolExecuteBeforeOutput,
};
#[cfg(feature = "wasm")]
use crate::wasm::WasmPluginRuntime;

#[cfg(feature = "wasm")]
struct WasmSessionHook {
    plugin_id: String,
    module: Module,
    exports: WasmHookExports,
}

#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Copy, Default)]
struct WasmHookExports {
    chat_system_transform: bool,
    tool_execute_before: bool,
    tool_execute_after: bool,
    session_compacting: bool,
}

#[cfg(feature = "wasm")]
impl WasmHookExports {
    fn from_module(module: &Module) -> Self {
        let names: std::collections::HashSet<&str> = module.exports().map(|e| e.name()).collect();
        Self {
            chat_system_transform: names.contains("chat_system_transform"),
            tool_execute_before: names.contains("tool_execute_before"),
            tool_execute_after: names.contains("tool_execute_after"),
            session_compacting: names.contains("session_compacting"),
        }
    }

    fn has_any(self) -> bool {
        self.chat_system_transform
            || self.tool_execute_before
            || self.tool_execute_after
            || self.session_compacting
    }
}

#[cfg(feature = "wasm")]
#[async_trait]
impl SessionHook for WasmSessionHook {
    fn id(&self) -> &str {
        &self.plugin_id
    }

    async fn chat_system_transform(
        &self,
        input: &ChatSystemTransformInput,
        output: &mut ChatSystemTransformOutput,
    ) -> Result<()> {
        if !self.exports.chat_system_transform {
            return Ok(());
        }
        let module = self.module.clone();
        let plugin_id = self.plugin_id.clone();
        let model = input.model.clone();
        let system_lines = output.system.clone();
        let transformed = tokio::task::spawn_blocking(move || {
            WasmPluginRuntime::transform_system_prompt(&module, &plugin_id, &system_lines, &model)
        })
        .await
        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))??;
        if let Some(lines) = transformed {
            output.system = lines;
        }
        Ok(())
    }

    async fn tool_execute_before(
        &self,
        input: &ToolExecuteBeforeInput,
        output: &mut ToolExecuteBeforeOutput,
    ) -> Result<()> {
        if !self.exports.tool_execute_before {
            return Ok(());
        }
        let module = self.module.clone();
        let plugin_id = self.plugin_id.clone();
        let payload = serde_json::json!({
            "tool": input.tool,
            "session_id": input.session_id,
            "call_id": input.call_id,
            "args": output.args,
        });
        let result = tokio::task::spawn_blocking(move || {
            WasmPluginRuntime::invoke_json_hook(
                &module,
                &plugin_id,
                "tool_execute_before",
                &payload,
            )
        })
        .await
        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))??;
        if let Some(value) = result {
            if let Some(args) = value.get("args") {
                output.args = args.clone();
            }
        }
        Ok(())
    }

    async fn tool_execute_after(
        &self,
        input: &ToolExecuteAfterInput,
        output: &mut ToolExecuteAfterOutput,
    ) -> Result<()> {
        if !self.exports.tool_execute_after {
            return Ok(());
        }
        let module = self.module.clone();
        let plugin_id = self.plugin_id.clone();
        let payload = serde_json::json!({
            "tool": input.tool,
            "session_id": input.session_id,
            "call_id": input.call_id,
            "args": input.args,
            "title": output.title,
            "output": output.output,
            "metadata": output.metadata,
        });
        let result = tokio::task::spawn_blocking(move || {
            WasmPluginRuntime::invoke_json_hook(&module, &plugin_id, "tool_execute_after", &payload)
        })
        .await
        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))??;
        if let Some(value) = result {
            if let Some(title) = value.get("title").and_then(|v| v.as_str()) {
                output.title = title.to_string();
            }
            if let Some(tool_output) = value.get("output") {
                output.output = tool_output.clone();
            }
            if let Some(metadata) = value.get("metadata") {
                output.metadata = metadata.clone();
            }
        }
        Ok(())
    }

    async fn session_compacting(
        &self,
        input: &SessionCompactingInput,
        output: &mut SessionCompactingOutput,
    ) -> Result<()> {
        if !self.exports.session_compacting {
            return Ok(());
        }
        let module = self.module.clone();
        let plugin_id = self.plugin_id.clone();
        let payload = serde_json::json!({
            "session_id": input.session_id,
            "context": output.context,
            "prompt": output.prompt,
        });
        let result = tokio::task::spawn_blocking(move || {
            WasmPluginRuntime::invoke_json_hook(&module, &plugin_id, "session_compacting", &payload)
        })
        .await
        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))??;
        if let Some(value) = result {
            if let Some(prompt) = value.get("prompt").and_then(|v| v.as_str()) {
                output.prompt = Some(prompt.to_string());
            }
            if let Some(context) = value.get("context").and_then(|v| v.as_array()) {
                output.context = context
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
            }
        }
        Ok(())
    }
}

pub trait CodePlusPlusPlugin: Send + Sync {
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
    plugins: Vec<Box<dyn CodePlusPlusPlugin>>,
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

    pub fn register(&mut self, plugin: Box<dyn CodePlusPlusPlugin>) {
        let _ = plugin.on_load();
        self.plugins.push(plugin);
    }

    pub fn load_wasm_dir(&mut self, dir: &Path) -> Result<()> {
        self.load_wasm_dir_with_trust(dir, &crate::verify::PluginTrust::default())
    }

    /// Load with explicit trust policy (see [`crate::verify::PluginTrust`]).
    /// Every `.wasm` file passes signature/pin verification before it is
    /// even validated as a module.
    pub fn load_wasm_dir_with_trust(
        &mut self,
        dir: &Path,
        trust: &crate::verify::PluginTrust,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        #[cfg(not(feature = "wasm"))]
        {
            let _ = trust;
            // Fail closed: silently skipping user-provided .wasm files would
            // hide a misconfigured slim build.
            let has_wasm = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wasm"))
                })
                .unwrap_or(false);
            if has_wasm {
                return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                    "wasm plugins are disabled in this build (found .wasm in {})",
                    dir.display()
                )));
            }
            return Ok(());
        }
        #[cfg(feature = "wasm")]
        {
            // Pins live next to the plugin dir's project metadata.
            let pins_dir = dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dir.to_path_buf());
            let engine = WasmPluginRuntime::create_engine();
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.path().extension().and_then(|e| e.to_str()) == Some("wasm") {
                    let path = entry.path();
                    crate::verify::verify_wasm_file(&path, &pins_dir, trust)?;
                    WasmPluginRuntime::validate(&path)?;
                    let module = Module::from_file(&engine, &path)
                        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))?;
                    // `file_name` is None only for paths ending in `..`/root;
                    // fail closed instead of panicking on a hostile dir entry.
                    let stem = path.file_name().ok_or_else(|| {
                        code_plus_plus_types::CodePlusPlusError::Other(format!(
                            "invalid wasm plugin path (no file name): {}",
                            path.display()
                        ))
                    })?;
                    self.wasm_modules
                        .insert(stem.to_string_lossy().to_string(), module);
                }
            }
            Ok(())
        }
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
                        target: code_plus_plus_telemetry::APP,
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
            Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                "not a native plugin library: {}",
                path.display()
            )))
        }
    }

    /// Load a native (`.dll`/`.so`/`.dylib`) plugin.
    ///
    /// Prefer **WASM plugins** (`load_wasm_dir_with_trust`, signature-pinned
    /// via [`crate::verify::verify_wasm_file`]): they run sandboxed in
    /// wasmtime with fuel metering. Native plugins run in-process with full
    /// process privileges — enable them only for explicitly trusted local
    /// libraries. NOTE: unlike WASM, native libraries have **no default
    /// signature verification**; treat `load_native_dir` as allow-listed local
    /// trust (see Policy `plugins` rules, which take precedence).
    fn load_native_plugin(&mut self, path: &Path) -> Result<()> {
        // SAFETY: `Library::new` + symbol calls execute third-party machine
        // code in-process; plugin init must not unwind across the FFI
        // boundary. Callers must only pass explicitly trusted local paths.
        unsafe {
            let library = Library::new(path).map_err(|e| {
                code_plus_plus_types::CodePlusPlusError::Other(format!(
                    "failed to load native plugin {}: {e}",
                    path.display()
                ))
            })?;

            let init: Symbol<unsafe extern "C" fn() -> i32> =
                library.get(b"code_plus_plus_plugin_init").map_err(|e| {
                    code_plus_plus_types::CodePlusPlusError::Other(format!(
                        "native plugin {} missing code_plus_plus_plugin_init: {e}",
                        path.display()
                    ))
                })?;
            if init() != 0 {
                return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                    "native plugin {} init failed",
                    path.display()
                )));
            }

            let id_fn: Symbol<unsafe extern "C" fn() -> *const u8> =
                library.get(b"code_plus_plus_plugin_id").map_err(|e| {
                    code_plus_plus_types::CodePlusPlusError::Other(format!(
                        "native plugin {} missing code_plus_plus_plugin_id: {e}",
                        path.display()
                    ))
                })?;
            // Validate the raw pointer before `CStr::from_ptr` (null or
            // unterminated/long data from a buggy/malicious plugin must not
            // cause UB or unbounded reads): null-check + bounded length scan.
            let raw = id_fn();
            if raw.is_null() {
                return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                    "native plugin {} returned null plugin id",
                    path.display()
                )));
            }
            const MAX_PLUGIN_ID_LEN: usize = 256;
            let len = (0..MAX_PLUGIN_ID_LEN)
                .find(|&i| *raw.add(i) == 0)
                .ok_or_else(|| {
                    code_plus_plus_types::CodePlusPlusError::Other(format!(
                        "native plugin {} plugin id not NUL-terminated within {MAX_PLUGIN_ID_LEN} bytes",
                        path.display()
                    ))
                })?;
            let id_bytes = std::slice::from_raw_parts(raw, len);
            let plugin_id = String::from_utf8_lossy(id_bytes).into_owned();

            tracing::info!(
                target: code_plus_plus_telemetry::APP,
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
        self.load_from_config_with_trust(
            plugins,
            project_dir,
            &crate::verify::PluginTrust::default(),
        )
    }

    /// Load with explicit trust policy (see [`crate::verify::PluginTrust`]).
    pub fn load_from_config_with_trust(
        &mut self,
        plugins: &[String],
        project_dir: &str,
        trust: &crate::verify::PluginTrust,
    ) -> Result<()> {
        let local_dir = Path::new(project_dir).join(".code-plus-plus").join("plugins");
        self.load_wasm_dir_with_trust(&local_dir, trust)?;
        self.load_native_dir(&local_dir)?;

        for entry in plugins {
            let path = Path::new(entry);
            if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                #[cfg(not(feature = "wasm"))]
                {
                    return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                        "wasm plugins are disabled in this build ({entry} not loaded)"
                    )));
                }
                #[cfg(feature = "wasm")]
                {
                    crate::verify::verify_wasm_file(
                        path,
                        &crate::verify::pins_dir_for(project_dir, path),
                        trust,
                    )?;
                    WasmPluginRuntime::validate(path)?;
                    let engine = WasmPluginRuntime::create_engine();
                    let module = Module::from_file(&engine, path)
                        .map_err(|e| code_plus_plus_types::CodePlusPlusError::Other(e.to_string()))?;
                    let stem = path.file_name().ok_or_else(|| {
                        code_plus_plus_types::CodePlusPlusError::Other(format!(
                            "invalid wasm plugin path (no file name): {}",
                            path.display()
                        ))
                    })?;
                    self.wasm_modules
                        .insert(stem.to_string_lossy().to_string(), module);
                }
            } else if Self::is_native_plugin(path) {
                self.load_native_plugin(path)?;
            } else if entry == "hello-hook" || entry.ends_with("hello-hook") {
                tracing::info!(
                    target: code_plus_plus_telemetry::APP,
                    plugin = %entry,
                    "registered built-in hello-hook"
                );
                self.register_hello_hook();
            } else {
                tracing::info!(
                    target: code_plus_plus_telemetry::APP,
                    plugin = %entry,
                    "native plugin loading deferred; use .dll/.so in .code-plus-plus/plugins/"
                );
            }
        }
        Ok(())
    }

    pub fn register_hooks(&self, registry: &mut HookRegistry) {
        for hook in &self.native_hooks {
            registry.register(hook.clone());
        }
        #[cfg(feature = "wasm")]
        for (name, module) in &self.wasm_modules {
            let exports = WasmHookExports::from_module(module);
            if !exports.has_any() {
                continue;
            }
            registry.register(Arc::new(WasmSessionHook {
                plugin_id: name.clone(),
                module: module.clone(),
                exports,
            }));
        }
    }

    pub fn list(&self) -> Vec<String> {
        let base = self
            .plugins
            .iter()
            .map(|p| p.id().to_string())
            .chain(self.native_hooks.iter().map(|h| h.id().to_string()));
        #[cfg(feature = "wasm")]
        {
            base.chain(self.wasm_modules.iter().filter_map(|(name, module)| {
                if WasmHookExports::from_module(module).has_any() {
                    Some(name.clone())
                } else {
                    None
                }
            }))
            .collect()
        }
        #[cfg(not(feature = "wasm"))]
        {
            // `wasm_modules` is always empty without wasmtime; keep the shape.
            let _ = &self.wasm_modules;
            base.collect()
        }
    }

    pub fn invoke_wasm(&self, name: &str) -> Result<bool> {
        self.invoke_wasm_export(name, "_initialize")
            .map(|_| true)
            .or_else(|_| Ok(self.wasm_modules.contains_key(name)))
    }

    pub fn invoke_wasm_export(&self, name: &str, export: &str) -> Result<()> {
        #[cfg(not(feature = "wasm"))]
        {
            let _ = (name, export);
            let _ = &self.wasm_modules;
            return Err(code_plus_plus_types::CodePlusPlusError::Other(
                "wasm plugins are disabled in this build".into(),
            ));
        }
        #[cfg(feature = "wasm")]
        {
            let module = self.wasm_modules.get(name).ok_or_else(|| {
                code_plus_plus_types::CodePlusPlusError::NotFound(format!("wasm plugin {name}"))
            })?;
            WasmPluginRuntime::invoke(module, name, export)
        }
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}
