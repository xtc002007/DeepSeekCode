use crate::theme::ThemeTokens;
use gpui::*;

pub fn render_divider_horizontal(theme: &ThemeTokens) -> impl IntoElement {
    div()
        .w_full()
        .h(px(1.))
        .bg(theme.border_subtle)
}

pub fn render_divider_vertical(theme: &ThemeTokens) -> impl IntoElement {
    div()
        .w(px(1.))
        .h_full()
        .bg(theme.border_subtle)
}

pub fn render_spacer(theme: &ThemeTokens, height: Pixels) -> impl IntoElement {
    div().w_full().h(height).bg(theme.bg_base)
}
