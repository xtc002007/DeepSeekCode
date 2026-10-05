use super::radius::RadiusScale;
use super::spacing::SpacingScale;
use super::typography::TypographyScale;
use gpui::*;

#[derive(Clone, Debug)]
pub struct MarkdownTokens {
    pub text: Rgba,
    pub muted: Rgba,
    pub code_bg: Rgba,
    pub border: Rgba,
    pub link: Rgba,
    pub blockquote_border: Rgba,
}

#[derive(Clone, Debug)]
pub struct SyntaxTokens {
    pub default: Rgba,
    pub comment: Rgba,
    pub string: Rgba,
    pub number: Rgba,
    pub keyword: Rgba,
    pub type_: Rgba,
    pub function: Rgba,
    pub property: Rgba,
}

#[derive(Clone, Debug)]
pub struct DiffTokens {
    pub insert_bg: Hsla,
    pub delete_bg: Hsla,
    pub insert_gutter: Hsla,
    pub delete_gutter: Hsla,
    pub equal_gutter: Hsla,
    pub insert_side_bg: Hsla,
    pub delete_side_bg: Hsla,
}

#[derive(Clone, Debug)]
pub struct ButtonTokens {
    pub primary_bg: Rgba,
    pub primary_bg_hover: Rgba,
    pub primary_text: Rgba,
    pub secondary_bg: Rgba,
    pub secondary_bg_hover: Rgba,
    pub secondary_text: Rgba,
    pub secondary_border: Rgba,
    pub ghost_hover: Rgba,
    pub danger_bg: Rgba,
    pub danger_text: Rgba,
}

#[derive(Clone, Debug)]
pub struct ComponentTokens {
    pub button: ButtonTokens,
}

#[derive(Clone, Debug)]
pub struct ThemeTokens {
    pub bg_base: Rgba,
    pub bg_surface: Rgba,
    pub bg_elevated: Rgba,
    pub bg_overlay: Rgba,
    pub bg_hover: Rgba,
    pub bg_active: Rgba,
    pub bg_selected: Rgba,

    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub text_disabled: Rgba,
    pub text_link: Rgba,
    pub text_on_accent: Rgba,

    pub border_subtle: Rgba,
    pub border_faint: Rgba,
    pub border_default: Rgba,
    pub border_focus: Rgba,

    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_muted: Rgba,

    pub window_close_hover: Rgba,

    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub info: Rgba,

    pub tool_read: Rgba,
    pub tool_write: Rgba,
    pub tool_bash: Rgba,

    pub terminal_bg: Rgba,
    pub terminal_fg: Rgba,
    /// Blinking block cursor (VSCode `terminalCursor.foreground`).
    pub terminal_cursor: Rgba,
    /// Typed command echo (VSCode PowerShell `dir` yellow).
    pub terminal_command: Rgba,
    /// Prompt prefix (`PS …>` / `$`) dimmed foreground.
    pub terminal_prompt: Rgba,
    /// Selection / active-tab underline accent for terminal tabs.
    pub terminal_tab_active: Rgba,

    pub markdown: MarkdownTokens,
    pub syntax: SyntaxTokens,
    pub diff: DiffTokens,
    pub components: ComponentTokens,

    /// Subtle drop shadow for elevated surfaces (light theme).
    pub shadow_elevated: Option<Hsla>,

    pub typography: TypographyScale,
    pub spacing: SpacingScale,
    pub radius: RadiusScale,
}

impl ThemeTokens {
    pub fn accent_rgba(&self) -> Rgba {
        self.accent.into()
    }

    pub fn accent_hover_rgba(&self) -> Rgba {
        self.accent_hover.into()
    }
}
