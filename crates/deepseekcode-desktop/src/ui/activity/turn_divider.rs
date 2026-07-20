use crate::theme::ThemeTokens;
use gpui::*;

pub fn render_turn_divider(theme: &ThemeTokens) -> impl IntoElement {
    let theme = theme.clone();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(2.))
        .py(px(2.))
        .child(
            div()
                .flex_1()
                .h(px(1.))
                .bg(theme.border_subtle),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child("·"),
        )
        .child(
            div()
                .flex_1()
                .h(px(1.))
                .bg(theme.border_subtle),
        )
}
