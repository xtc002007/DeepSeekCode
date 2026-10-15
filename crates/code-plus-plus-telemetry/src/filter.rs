use code_plus_plus_config::LoggingConfig;
use std::env;

/// Build the `EnvFilter` directive string from config + environment overrides.
pub fn build_filter_string(config: &LoggingConfig) -> String {
    if let Ok(rust_log) = env::var("RUST_LOG") {
        if !rust_log.trim().is_empty() {
            return rust_log;
        }
    }

    let level = env::var("CODE_PLUS_PLUS_LOG_LEVEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config.level.clone());

    // P1-13: real targets use `::` (`code-plus-plus::app`), not crate
    // underscores. Emit both the top-level prefix and the actual module
    // targets so per-module tuning works.
    // Product-grade fix: cover EVERY canonical subsystem target. Previously
    // only app/session/tool/audit were emitted, so llm/storage/server/
    // desktop/cli/permission fell back to the bare `code-plus-plus=` prefix
    // and could be silently mistuned when users set per-target overrides.
    let mut parts = vec![
        format!("code-plus-plus={level}"),
        format!("{}={level}", crate::targets::APP),
        format!("{}={level}", crate::targets::SESSION),
        format!("{}={level}", crate::targets::TOOL),
        format!("{}={level}", crate::targets::AUDIT),
        format!("{}={level}", crate::targets::LLM),
        format!("{}={level}", crate::targets::STORAGE),
        format!("{}={level}", crate::targets::SERVER),
        format!("{}={level}", crate::targets::DESKTOP),
        format!("{}={level}", crate::targets::CLI),
        format!("{}={level}", crate::targets::PERMISSION),
        // Crate-prefix fallback: events without an explicit `target:`
        // (module path like `code_plus_plus_core::engine`) must not be
        // silently dropped if `code-plus-plus=` prefix matching ever narrows.
        format!("code_plus_plus_core={level}"),
        format!("code_plus_plus_config={level}"),
        format!("code_plus_plus_llm={level}"),
        format!("code_plus_plus_storage={level}"),
        format!("code_plus_plus_embed={level}"),
        format!("code_plus_plus_sdk={level}"),
        format!("code_plus_plus_plugin={level}"),
        format!("code_plus_plus_telemetry={level}"),
        format!("code_plus_plus_server={level}"),
        format!("code_plus_plus_cli={level}"),
        format!("code_plus_plus_desktop={level}"),
    ];

    for (module, module_level) in &config.modules {
        parts.push(format!("{module}={module_level}"));
    }

    parts.join(",")
}

pub fn resolve_logs_root(config: &LoggingConfig) -> std::path::PathBuf {
    if let Ok(dir) = env::var("CODE_PLUS_PLUS_LOG_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    if let Some(dir) = &config.dir {
        return std::path::PathBuf::from(dir);
    }
    code_plus_plus_types::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("code-plus-plus")
        .join("logs")
}

pub fn resolve_console_enabled(config: &LoggingConfig) -> bool {
    if let Ok(v) = env::var("CODE_PLUS_PLUS_LOG_CONSOLE") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    config.console
}
