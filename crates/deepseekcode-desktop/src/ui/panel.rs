use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub fn render_panel_header(
    title: impl Into<SharedString>,
    theme: &ThemeTokens,
    actions: impl IntoIterator<Item = AnyElement>,
) -> impl IntoElement {
    let theme = theme.clone();
    div()
        .w_full()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(title.into()),
        )
        .child(div().flex().gap(px(4.)).children(actions))
}

pub fn render_panel_header_icon_button<F>(
    id: impl Into<ElementId>,
    icon: IconName,
    theme: &ThemeTokens,
    on_click: F,
) -> impl IntoElement
where
    F: Fn(&mut App) + 'static,
{
    let theme = theme.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_sm()
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
        .child(render_icon_rgba(icon, px(14.), theme.text_secondary))
}
