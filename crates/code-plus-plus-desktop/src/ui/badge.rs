use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

/// Badge shell — attach `.on_mouse_up(cx.listener(...))` when clickable.
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
        // P2: active hover stays in the selected family so hover never reads
        // as "deselected".
        let hover_bg = if active {
            theme.bg_selected
        } else {
            theme.bg_hover
        };
        el = el.cursor_pointer().hover(move |s| s.bg(hover_bg));
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
    let _clickable = on_click.is_some();
    if let Some(handler) = on_click {
        let shell = badge_shell(id, label, theme, icon, active, true);
        // P1-1 release-to-confirm + P1-2 stop.
        shell
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                handler(cx)
            })
            .into_any_element()
    } else {
        // P2: display-only badges use a plain div (no stateful id/focus ring,
        // no wasted state node).
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
        if let Some(icon_name) = icon {
            el = el.child(render_icon_rgba(icon_name, px(12.), text));
        }
        el.child(label).into_any_element()
    }
}

/// Small status pill (e.g. git M/U/D).
pub fn render_status_badge(
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    color: Rgba,
) -> impl IntoElement {
    let label = label.into();
    // P2: guarantee contrast on light themes — mix the status color toward the
    // primary text when the raw color is too light.
    let fg = {
        let lum = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
        if lum > 0.75 { theme.text_primary } else { color }
    };
    div()
        .px_1()
        .py(px(1.))
        .text_xs()
        .rounded_sm()
        .bg(color.opacity(0.2))
        .text_color(fg)
        .child(label)
}
