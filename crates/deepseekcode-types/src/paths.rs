//! OS standard directories (single source of truth).
//!
//! Backed by the maintained `directories` crate (`BaseDirs`), replacing the
//! unmaintained `dirs` shims previously scattered across seven crates with
//! subtly different fallbacks. Semantics are unchanged on all platforms
//! (Windows `%APPDATA%`, XDG on Linux, `~/Library` on macOS); sandboxed
//! environments without a home still yield `None` and callers keep their
//! existing fallbacks.

use std::path::PathBuf;
use std::sync::OnceLock;

fn base_dirs() -> Option<&'static directories::BaseDirs> {
    static BASE: OnceLock<Option<directories::BaseDirs>> = OnceLock::new();
    BASE.get_or_init(directories::BaseDirs::new).as_ref()
}

/// Roaming config root (`%APPDATA%` / `$XDG_CONFIG_HOME` / `~/Library/...`).
pub fn config_dir() -> Option<PathBuf> {
    base_dirs().map(|b| b.config_dir().to_path_buf())
}

/// Local data root for databases, logs, and caches.
pub fn data_dir() -> Option<PathBuf> {
    base_dirs().map(|b| b.data_dir().to_path_buf())
}

/// Current user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    base_dirs().map(|b| b.home_dir().to_path_buf())
}
