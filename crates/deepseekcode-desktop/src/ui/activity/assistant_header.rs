use crate::theme::ThemeTokens;
use crate::ui::copy_button::render_copy_button;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub fn render_assistant_header(leaf_id: &str, text: &str, theme: &ThemeTokens) -> impl IntoElement {
    let theme = theme.clone();
    let copy_text = text.to_string();
    let copy_id = ElementId::Name(SharedString::from(format!("copy-assistant-{leaf_id}")));
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(render_icon_rgba(IconName::Bot, px(14.), theme.info))
                .child(
                    div()
                        .text_xs()
                        .line_height(crate::theme::leading_xs(&theme))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child("Assistant"),
                ),
        )
        .child(render_copy_button(copy_id, copy_text, &theme))
}
