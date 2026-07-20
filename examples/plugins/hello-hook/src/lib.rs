//! Minimal native plugin example for DeepSeekCode hook registration.
//!
//! Build: `cargo build -p hello-hook` from workspace root after adding to members,
//! or `cargo build` from this directory.

/// Plugin entry — returns 0 on success.
#[no_mangle]
pub extern "C" fn deepseekcode_plugin_init() -> i32 {
    0
}

/// Returns a static plugin id string.
#[no_mangle]
pub extern "C" fn deepseekcode_plugin_id() -> *const u8 {
    b"hello-hook\0".as_ptr()
}
