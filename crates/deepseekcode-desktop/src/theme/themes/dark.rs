use crate::theme::palette::{hex_hsla, hex_rgb};
use crate::theme::tokens::{
    ButtonTokens, ComponentTokens, DiffTokens, MarkdownTokens, SyntaxTokens, ThemeTokens,
};
use crate::theme::{RadiusScale, SpacingScale, TypographyScale};

pub fn dark_tokens() -> ThemeTokens {
    ThemeTokens {
        bg_base: hex_rgb(0x0d0d0d),
        bg_surface: hex_rgb(0x161616),
        bg_elevated: hex_rgb(0x1e1e1e),
        bg_overlay: hex_rgb(0x252525),
        bg_hover: hex_rgb(0x222222),
        bg_active: hex_rgb(0x2a2a2a),
        bg_selected: hex_rgb(0x2a3344),

        text_primary: hex_rgb(0xd4d4d4),
        text_secondary: hex_rgb(0x9d9d9d),
        text_muted: hex_rgb(0x858585),
        text_disabled: hex_rgb(0x666666),
        text_link: hex_rgb(0x7eb8da),
        text_on_accent: hex_rgb(0xffffff),

        border_subtle: hex_rgb(0x2a2a2a),
        border_faint: hex_rgb(0x222222),
        border_default: hex_rgb(0x333333),
        border_focus: hex_rgb(0x4a6a9a),

        accent: hex_hsla(0.58, 0.7, 0.55, 1.0),
        accent_hover: hex_hsla(0.58, 0.7, 0.48, 1.0),
        accent_muted: hex_rgb(0x1e2433),
        window_close_hover: hex_rgb(0xe81123),

        success: hex_rgb(0x4ade80),
        warning: hex_rgb(0xfbbf24),
        error: hex_rgb(0xef4444),
        info: hex_rgb(0x60a5fa),

        tool_read: hex_rgb(0x60a5fa),
        tool_write: hex_rgb(0x4ade80),
        tool_bash: hex_rgb(0xfbbf24),

        // VSCode Dark+ terminal: #1E1E1E bg, #CCCCCC fg.
        terminal_bg: hex_rgb(0x1e1e1e),
        terminal_fg: hex_rgb(0xcccccc),
        terminal_cursor: hex_rgb(0xaeafad),
        terminal_command: hex_rgb(0xf9f1a5),
        terminal_prompt: hex_rgb(0x858585),
        terminal_tab_active: hex_rgb(0x007acc),

        markdown: MarkdownTokens {
            text: hex_rgb(0xe0e0e0),
            muted: hex_rgb(0x888888),
            code_bg: hex_rgb(0x1e1e1e),
            border: hex_rgb(0x333333),
            link: hex_rgb(0x7eb8da),
            blockquote_border: hex_rgb(0x4a6a9a),
        },

        syntax: SyntaxTokens {
            default: hex_rgb(0xd4d4d4),
            comment: hex_rgb(0x6a9955),
            string: hex_rgb(0xce9178),
            number: hex_rgb(0xb5cea8),
            keyword: hex_rgb(0x569cd6),
            type_: hex_rgb(0x4ec9b0),
            function: hex_rgb(0xdcdcaa),
            property: hex_rgb(0x9cdcfe),
        },

        diff: DiffTokens {
            delete_bg: hex_hsla(0.0, 0.4, 0.15, 1.0),
            insert_bg: hex_hsla(0.33, 0.4, 0.15, 1.0),
            delete_gutter: hex_hsla(0.0, 0.5, 0.75, 1.0),
            insert_gutter: hex_hsla(0.33, 0.5, 0.75, 1.0),
            equal_gutter: hex_hsla(0.0, 0.0, 0.85, 1.0),
            delete_side_bg: hex_hsla(0.0, 0.4, 0.15, 1.0),
            insert_side_bg: hex_hsla(0.33, 0.4, 0.15, 1.0),
        },

        components: ComponentTokens {
            button: ButtonTokens {
                primary_bg: hex_hsla(0.58, 0.7, 0.55, 1.0).into(),
                primary_bg_hover: hex_hsla(0.58, 0.7, 0.48, 1.0).into(),
                primary_text: hex_rgb(0xffffff),
                secondary_bg: hex_rgb(0x1e1e1e),
                secondary_bg_hover: hex_rgb(0x222222),
                secondary_text: hex_rgb(0xe0e0e0),
                secondary_border: hex_rgb(0x333333),
                ghost_hover: hex_rgb(0x222222),
                danger_bg: hex_rgb(0xef4444),
                danger_text: hex_rgb(0xffffff),
            },
        },

        shadow_elevated: None,

        typography: TypographyScale::default(),
        spacing: SpacingScale::default(),
        radius: RadiusScale::default(),
    }
}
