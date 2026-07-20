//! File-type icon and color helpers.

use crate::theme::ThemeTokens;
use crate::ui::icon::IconName;
use gpui::Rgba;

pub fn icon_for_path(path: &str) -> IconName {
    if std::path::Path::new(path).is_dir() {
        return IconName::Folder;
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "rs" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "py" | "pyw" => IconName::Code,
        "json" | "jsonc" | "toml" | "yaml" | "yml" => IconName::List,
        "md" | "markdown" => IconName::BookOpen,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => IconName::Eye,
        "lock" => IconName::CircleDot,
        _ => IconName::File,
    }
}

pub fn color_for_path(path: &str, theme: &ThemeTokens) -> Rgba {
    if std::path::Path::new(path).is_dir() {
        return theme.warning;
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "rs" => theme.warning,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => theme.info,
        "py" | "pyw" => theme.success,
        "json" | "jsonc" | "toml" | "yaml" | "yml" => theme.warning,
        "md" | "markdown" => theme.text_secondary,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => theme.tool_read,
        _ => theme.text_secondary,
    }
}
