//! User-visible product naming. Use [`PRODUCT_NAME`] in UI, errors, and docs prose.
//! On-disk paths and CLI invocations stay lowercase (see [`CONFIG_FILE`], [`CLI_NAME`]).

/// Display name shown to users (window titles, about text, error messages).
pub const PRODUCT_NAME: &str = "Code++";

/// Project config filename on disk.
pub const CONFIG_FILE: &str = "code-plus-plus.jsonc";

/// Project-local runtime directory.
pub const LOCAL_DIR: &str = ".code-plus-plus";

/// CLI / executable base name (lowercase).
pub const CLI_NAME: &str = "code-plus-plus";

/// Default diagnostics archive prefix (user-visible download name).
pub const DIAGNOSTICS_ARCHIVE_PREFIX: &str = "CodePlusPlus-diagnostics";

/// Default session export filename prefix.
pub const SESSION_EXPORT_PREFIX: &str = "CodePlusPlus-export";
