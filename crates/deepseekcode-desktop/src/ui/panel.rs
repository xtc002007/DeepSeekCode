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
                // P2: muted xs on elevated can fall below a11y contrast —
                // use secondary for the header title.
                .text_color(theme.text_secondary)
                .child(title.into()),
        )
        // P2: center heterogeneous icons vertically; 8px gap meets the 32px
        // touch pitch with 24px targets.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(actions),
        )
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
        .active(|s| s.bg(theme.bg_active))
        // P1-1 release-to-confirm; P1-2 stop so a header ✕ never starts a
        // panel drag/resize underneath.
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            on_click(cx)
        })
        .child(render_icon_rgba(icon, px(14.), theme.text_secondary))
}
