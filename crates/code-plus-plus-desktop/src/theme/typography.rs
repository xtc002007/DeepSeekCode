use gpui::px;
use gpui::Pixels;
use gpui::SharedString;

/// Compact typography aligned with Cursor IDE density (~1.33–1.4 line-height).
#[derive(Clone, Debug)]
pub struct TypographyScale {
    pub font_ui: SharedString,
    pub font_mono: SharedString,
    pub text_xs: Pixels,
    pub text_sm: Pixels,
    pub text_base: Pixels,
    pub text_md: Pixels,
    pub text_lg: Pixels,
    /// Labels / section headers (11px → 14px).
    pub leading_xs: Pixels,
    /// Nav lists & compact rows (12px → 16px).
    pub leading_sm: Pixels,
    /// Body copy & input (13px → 18px).
    pub leading_base: Pixels,
    /// Markdown paragraphs (13px → 20px).
    pub leading_relaxed: Pixels,
}

impl Default for TypographyScale {
    fn default() -> Self {
        Self {
            // UI stack: system font first, then CJK + Cyrillic coverage.
            // Windows: Microsoft YaHei / SimSun; macOS: PingFang / Hiragino;
            // Linux: Noto CJK. Cyrillic is covered by Segoe UI / system fonts,
            // kept explicit so a missing primary never falls back to tofu.
            font_ui: ".SystemUIFont, Segoe UI, Microsoft YaHei, PingFang SC, Hiragino Sans GB, Noto Sans CJK SC, Noto Sans, sans-serif".into(),
            // Mono stack: Latin mono first, then CJK mono, then generic.
            // The previous `"Cascadia Mono, Consolas, monospace"` had no Han
            // glyphs, so CJK fell back unpredictably per machine.
            font_mono: "Cascadia Mono, Consolas, Microsoft YaHei UI, PingFang SC, Hiragino Sans GB, Noto Sans Mono CJK SC, Noto Sans Mono, monospace".into(),
            text_xs: px(11.),
            text_sm: px(12.),
            text_base: px(13.),
            text_md: px(14.),
            text_lg: px(16.),
            leading_xs: px(14.),
            leading_sm: px(16.),
            leading_base: px(18.),
            leading_relaxed: px(20.),
        }
    }
}
