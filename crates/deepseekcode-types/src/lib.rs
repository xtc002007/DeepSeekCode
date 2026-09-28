pub mod branding;
pub mod error;
pub mod ids;
pub mod paths;
pub mod tool_arguments;

pub use branding::{
    CLI_NAME, CONFIG_FILE, DIAGNOSTICS_ARCHIVE_PREFIX, LEGACY_CONFIG_FILE, LOCAL_DIR, PRODUCT_NAME,
    SESSION_EXPORT_PREFIX,
};
pub use error::{DeepSeekCodeError, Result};
pub use ids::*;
pub use paths::{config_dir, data_dir, home_dir};
pub use tool_arguments::{
    normalize_tool_arguments, normalize_tool_arguments_for_fingerprint, parse_tool_arguments,
    tool_fingerprint_canonical,
};
