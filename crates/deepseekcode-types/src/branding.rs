//! User-visible product naming. Use [`PRODUCT_NAME`] in UI, errors, and docs prose.
//! On-disk paths and CLI invocations stay lowercase (see [`CONFIG_FILE`], [`CLI_NAME`]).

/// Display name shown to users (window titles, about text, error messages).
pub const PRODUCT_NAME: &str = "DeepSeekCode";

/// Project config filename on disk.
pub const CONFIG_FILE: &str = "deepseekcode.jsonc";

/// Legacy config filename (migration).
pub const LEGACY_CONFIG_FILE: &str = "codevibe.jsonc";

/// Project-local runtime directory.
pub const LOCAL_DIR: &str = ".deepseekcode";

/// CLI / executable base name (lowercase).
pub const CLI_NAME: &str = "deepseekcode";

/// Default diagnostics archive prefix (user-visible download name).
pub const DIAGNOSTICS_ARCHIVE_PREFIX: &str = "DeepSeekCode-diagnostics";

/// Default session export filename prefix.
pub const SESSION_EXPORT_PREFIX: &str = "DeepSeekCode-export";
