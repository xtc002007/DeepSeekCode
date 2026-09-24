pub mod branding;
pub mod error;
pub mod ids;
pub mod tool_arguments;

pub use branding::{
    CLI_NAME, CONFIG_FILE, DIAGNOSTICS_ARCHIVE_PREFIX, LEGACY_CONFIG_FILE, LOCAL_DIR, PRODUCT_NAME,
    SESSION_EXPORT_PREFIX,
};
pub use error::{DeepSeekCodeError, Result};
pub use ids::*;
pub use tool_arguments::{
    normalize_tool_arguments, parse_tool_arguments, tool_fingerprint_canonical,
};
