//! Minimal native plugin example for Code++ hook registration.
//!
//! Build: `cargo build -p hello-hook` from workspace root after adding to members,
//! or `cargo build` from this directory.

/// Plugin entry — returns 0 on success.
#[no_mangle]
pub extern "C" fn code_plus_plus_plugin_init() -> i32 {
    0
}

/// Returns a static plugin id string.
// `*const u8` ABI: kept as a byte string (a `c""` literal yields `*const
// c_char`, which mismatches on platforms where `c_char = i8`).
#[allow(clippy::manual_c_str_literals)]
#[no_mangle]
pub extern "C" fn code_plus_plus_plugin_id() -> *const u8 {
    b"hello-hook\0".as_ptr()
}
