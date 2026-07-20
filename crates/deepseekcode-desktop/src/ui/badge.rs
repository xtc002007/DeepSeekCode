use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon, render_icon_rgba, IconName};
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Badge shell — attach `.on_mouse_down(cx.listener(...))` when clickable.
pub fn badge_shell(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    icon: Option<IconName>,
    active: bool,
    clickable: bool,
) -> Stateful<Div> {
    let label = label.into();
    let theme = theme.clone();
    let bg = if active {
        theme.bg_selected
    } else {
        theme.bg_elevated
    };
    let text = if active {
        theme.text_primary
    } else {
        theme.text_secondary
    };

    let mut el = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(6.))
        .py(px(3.))
        .rounded_md()
        .bg(bg)
        .border_1()
        .border_color(theme.border_subtle)
        .text_xs()
        .text_color(text);

    if clickable {
        let bg_hover = theme.bg_hover;
        el = el.cursor_pointer().hover(|s| s.bg(bg_hover));
    }

    if let Some(icon_name) = icon {
        el = el.child(render_icon_rgba(icon_name, px(12.), text));
    }

    el.child(label)
}

pub fn render_badge<F>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    icon: Option<IconName>,
    active: bool,
    on_click: Option<F>,
) -> impl IntoElement
where
    F: Fn(&mut App) + 'static,
{
    let clickable = on_click.is_some();
    let shell = badge_shell(id, label, theme, icon, active, clickable);
    if let Some(handler) = on_click {
        shell.on_mouse_down(MouseButton::Left, move |_, _, cx| handler(cx))
    } else {
        shell
    }
}

/// Small status pill (e.g. git M/U/D).
pub fn render_status_badge(
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    color: Rgba,
) -> impl IntoElement {
    let label = label.into();
    let theme = theme.clone();
    div()
        .px_1()
        .text_xs()
        .rounded_sm()
        .bg(color.opacity(0.2))
        .text_color(color)
        .child(label)
}
