//! Loads the hello-hook cdylib when present (built in CI via `cargo build -p hello-hook`).

use deepseekcode_plugin::PluginHost;
use std::path::PathBuf;

fn target_dir() -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

fn hello_hook_artifact() -> Option<PathBuf> {
    let file = if cfg!(windows) {
        "hello_hook.dll"
    } else if cfg!(target_os = "macos") {
        "libhello_hook.dylib"
    } else {
        "libhello_hook.so"
    };
    let base = target_dir();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    for profile in [profile.as_str(), "release", "debug"] {
        let path = base.join(profile).join(file);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

#[test]
fn native_hello_hook_loads_when_built() {
    let Some(path) = hello_hook_artifact() else {
        eprintln!("skip native_hello_hook_loads_when_built: hello-hook cdylib not built");
        return;
    };

    let mut host = PluginHost::new();
    host.load_native_file(&path).expect("load hello-hook native plugin");
    let plugins = host.list();
    assert!(
        plugins.iter().any(|p| p.contains("hello")),
        "expected hello-hook in plugin list: {plugins:?}"
    );
}
