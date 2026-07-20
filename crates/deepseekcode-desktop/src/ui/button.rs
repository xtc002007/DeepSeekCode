use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Sm,
    Md,
    Lg,
}

fn size_styles(size: ButtonSize) -> (Pixels, Pixels, Pixels) {
    match size {
        ButtonSize::Sm => (px(24.), px(8.), px(4.)),
        ButtonSize::Md => (px(32.), px(12.), px(6.)),
        ButtonSize::Lg => (px(40.), px(16.), px(8.)),
    }
}

fn variant_colors(
    variant: ButtonVariant,
    theme: &ThemeTokens,
) -> (Rgba, Rgba, Rgba, Option<Rgba>) {
    let btn = &theme.components.button;
    match variant {
        ButtonVariant::Primary => (
            btn.primary_bg,
            btn.primary_bg_hover,
            btn.primary_text,
            None,
        ),
        ButtonVariant::Secondary => (
            btn.secondary_bg,
            btn.secondary_bg_hover,
            btn.secondary_text,
            Some(btn.secondary_border),
        ),
        ButtonVariant::Ghost => (
            hsla(0., 0., 0., 0.).into(),
            btn.ghost_hover,
            theme.text_secondary,
            None,
        ),
        ButtonVariant::Danger => (btn.danger_bg, btn.danger_bg, btn.danger_text, None),
    }
}

/// Styled button shell — attach `.on_mouse_down(cx.listener(...))` at the call site.
pub fn button_shell(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    icon: Option<IconName>,
    focused: bool,
) -> Stateful<Div> {
    let label = label.into();
    let theme = theme.clone();
    let (min_h, px_h, py_v) = size_styles(size);
    let (bg, bg_hover, text, border) = variant_colors(variant, &theme);
    let border_focus = theme.border_focus;
    let bg_active = theme.bg_active;

    let mut el = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .min_h(min_h)
        .px(px_h)
        .py(py_v)
        .rounded_md()
        .bg(bg)
        .text_color(text)
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .when(disabled, |s| s.opacity(0.5).cursor_default())
        .when(!disabled, |s| {
            s.cursor_pointer()
                .hover(|s| s.bg(bg_hover))
                .active(|s| s.bg(bg_active))
        })
        .when(focused, |s| s.border_2().border_color(border_focus));

    if let Some(border) = border {
        el = el.border_1().border_color(border);
    }

    if let Some(icon_name) = icon {
        el = el.child(render_icon_rgba(icon_name, px(14.), text));
    }

    el.child(label)
}

pub fn render_button<F>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &ThemeTokens,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    icon: Option<IconName>,
    on_click: F,
) -> impl IntoElement
where
    F: Fn(&mut App) + 'static,
{
    let shell = button_shell(id, label, theme, variant, size, disabled, icon, false);
    if disabled {
        shell
    } else {
        shell.on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
    }
}

/// Icon-only button shell — attach `.on_mouse_down(cx.listener(...))` at the call site.
pub fn icon_button_shell(
    id: impl Into<ElementId>,
    icon: IconName,
    theme: &ThemeTokens,
    size: Pixels,
    active: bool,
    danger: bool,
    focused: bool,
) -> Stateful<Div> {
    let theme = theme.clone();
    let icon_color: Rgba = if danger {
        theme.error
    } else if active {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    let bg: Rgba = if active {
        theme.bg_selected
    } else {
        hsla(0., 0., 0., 0.).into()
    };
    let bg_hover = theme.bg_hover;
    let bg_active = theme.bg_active;
    let border_focus = theme.border_focus;

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(size)
        .rounded_sm()
        .bg(bg)
        .cursor_pointer()
        .hover(|s| s.bg(bg_hover))
        .active(|s| s.bg(bg_active))
        .when(focused, |s| s.border_2().border_color(border_focus))
        .child(render_icon_rgba(icon, px(16.), icon_color))
}

pub fn render_icon_button<F>(
    id: impl Into<ElementId>,
    icon: IconName,
    theme: &ThemeTokens,
    size: Pixels,
    active: bool,
    danger: bool,
    on_click: F,
) -> impl IntoElement
where
    F: Fn(&mut App) + 'static,
{
    icon_button_shell(id, icon, theme, size, active, danger, false)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
}
