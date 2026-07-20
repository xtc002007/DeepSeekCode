use deepseekcode_config::LoggingConfig;
use std::env;

/// Build the `EnvFilter` directive string from config + environment overrides.
pub fn build_filter_string(config: &LoggingConfig) -> String {
    if let Ok(rust_log) = env::var("RUST_LOG") {
        if !rust_log.trim().is_empty() {
            return rust_log;
        }
    }

    let level = env::var("DEEPSEEKCODE_LOG_LEVEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config.level.clone());

    let mut parts = vec![
        format!("deepseekcode={level}"),
        format!("deepseekcode_core={level}"),
        format!("deepseekcode_server={level}"),
        format!("deepseekcode_llm={level}"),
        format!("deepseekcode_storage={level}"),
        format!("deepseekcode_sdk={level}"),
        format!("deepseekcode_desktop={level}"),
        format!("deepseekcode_telemetry={level}"),
    ];

    for (module, module_level) in &config.modules {
        parts.push(format!("{module}={module_level}"));
    }

    parts.join(",")
}

pub fn resolve_logs_root(config: &LoggingConfig) -> std::path::PathBuf {
    if let Ok(dir) = env::var("DEEPSEEKCODE_LOG_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    if let Some(dir) = &config.dir {
        return std::path::PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("deepseekcode")
        .join("logs")
}

pub fn resolve_console_enabled(config: &LoggingConfig) -> bool {
    if let Ok(v) = env::var("DEEPSEEKCODE_LOG_CONSOLE") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    config.console
}
