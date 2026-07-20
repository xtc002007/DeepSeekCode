use crate::theme::palette::{hex_hsla, hex_rgb};
use crate::theme::tokens::{
    ButtonTokens, ComponentTokens, DiffTokens, MarkdownTokens, SyntaxTokens, ThemeTokens,
};
use crate::theme::{RadiusScale, SpacingScale, TypographyScale};

pub fn light_tokens() -> ThemeTokens {
    ThemeTokens {
        bg_base: hex_rgb(0xffffff),
        bg_surface: hex_rgb(0xf5f5f5),
        bg_elevated: hex_rgb(0xffffff),
        bg_overlay: hex_rgb(0xffffff),
        bg_hover: hex_rgb(0xeeeeee),
        bg_active: hex_rgb(0xe5e5e5),
        bg_selected: hex_rgb(0xdbeafe),

        text_primary: hex_rgb(0x1a1a1a),
        text_secondary: hex_rgb(0x4a4a4a),
        text_muted: hex_rgb(0x737373),
        text_disabled: hex_rgb(0xa3a3a3),
        text_link: hex_rgb(0x2563eb),
        text_on_accent: hex_rgb(0xffffff),

        border_subtle: hex_rgb(0xe5e5e5),
        border_faint: hex_rgb(0xf3f3f3),
        border_default: hex_rgb(0xd4d4d4),
        border_focus: hex_rgb(0x3b82f6),

        accent: hex_hsla(0.58, 0.7, 0.45, 1.0),
        accent_hover: hex_hsla(0.58, 0.7, 0.38, 1.0),
        accent_muted: hex_rgb(0xeef2ff),
        window_close_hover: hex_rgb(0xe81123),

        success: hex_rgb(0x16a34a),
        warning: hex_rgb(0xd97706),
        error: hex_rgb(0xdc2626),
        info: hex_rgb(0x2563eb),

        tool_read: hex_rgb(0x2563eb),
        tool_write: hex_rgb(0x16a34a),
        tool_bash: hex_rgb(0xd97706),

        terminal_bg: hex_rgb(0xfafafa),
        terminal_fg: hex_rgb(0x1a1a1a),

        markdown: MarkdownTokens {
            text: hex_rgb(0x1a1a1a),
            muted: hex_rgb(0x737373),
            code_bg: hex_rgb(0xf5f5f5),
            border: hex_rgb(0xd4d4d4),
            link: hex_rgb(0x2563eb),
            blockquote_border: hex_rgb(0x93c5fd),
        },

        syntax: SyntaxTokens {
            default: hex_rgb(0x24292e),
            comment: hex_rgb(0x6a737d),
            string: hex_rgb(0x032f62),
            number: hex_rgb(0x005cc5),
            keyword: hex_rgb(0xd73a49),
            type_: hex_rgb(0x6f42c1),
            function: hex_rgb(0x6f42c1),
            property: hex_rgb(0x005cc5),
        },

        diff: DiffTokens {
            delete_bg: hex_hsla(0.0, 0.7, 0.95, 1.0),
            insert_bg: hex_hsla(0.33, 0.7, 0.95, 1.0),
            delete_gutter: hex_hsla(0.0, 0.6, 0.45, 1.0),
            insert_gutter: hex_hsla(0.33, 0.6, 0.45, 1.0),
            equal_gutter: hex_hsla(0.0, 0.0, 0.55, 1.0),
            delete_side_bg: hex_hsla(0.0, 0.7, 0.95, 1.0),
            insert_side_bg: hex_hsla(0.33, 0.7, 0.95, 1.0),
        },

        components: ComponentTokens {
            button: ButtonTokens {
                primary_bg: hex_hsla(0.58, 0.7, 0.45, 1.0).into(),
                primary_bg_hover: hex_hsla(0.58, 0.7, 0.38, 1.0).into(),
                primary_text: hex_rgb(0xffffff),
                secondary_bg: hex_rgb(0xffffff),
                secondary_bg_hover: hex_rgb(0xeeeeee),
                secondary_text: hex_rgb(0x1a1a1a),
                secondary_border: hex_rgb(0xd4d4d4),
                ghost_hover: hex_rgb(0xeeeeee),
                danger_bg: hex_rgb(0xdc2626),
                danger_text: hex_rgb(0xffffff),
            },
        },

        shadow_elevated: Some(hex_hsla(0.0, 0.0, 0.0, 0.08)),

        typography: TypographyScale::default(),
        spacing: SpacingScale::default(),
        radius: RadiusScale::default(),
    }
}
