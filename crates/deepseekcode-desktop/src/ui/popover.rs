//! Workspace-level popover menus — opaque, topmost, auto-width.

use gpui::prelude::FluentBuilder;
use gpui::*;

use super::dropdown::{DropDirection, DropdownOption};

pub fn menu_width_for_labels(labels: impl IntoIterator<Item = impl AsRef<str>>) -> Pixels {
    let max_chars = labels
        .into_iter()
        .map(|s| s.as_ref().chars().count())
        .max()
        .unwrap_or(8);
    px((max_chars as f32 * 7.2 + 28.0).clamp(120.0, 420.0))
}

pub fn menu_width_for_options(options: &[DropdownOption]) -> Pixels {
    menu_width_for_labels(options.iter().map(|o| o.label.as_str()))
}

pub fn menu_top_for_direction(
    anchor: Point<Pixels>,
    direction: DropDirection,
    item_count: usize,
) -> Pixels {
    let row_h = px(28.);
    let padding = px(8.);
    let menu_h = row_h * item_count as f32 + padding;
    match direction {
        DropDirection::Down => anchor.y + px(4.),
        DropDirection::Up => (anchor.y - menu_h - px(4.)).max(px(4.)),
    }
}

pub fn render_menu_panel(
    options: &[DropdownOption],
    selected_id: &str,
    anchor: Point<Pixels>,
    direction: DropDirection,
    on_select: impl Fn(
            &mut crate::workspace::WorkspaceView,
            String,
            &mut Context<crate::workspace::WorkspaceView>,
        )
        + 'static
        + Clone,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let width = menu_width_for_options(options);
    let top = menu_top_for_direction(anchor, direction, options.len());

    let mut menu = div()
        .id("popover-menu-panel")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(top)
        .w(width)
        .max_h(px(320.))
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default);

    for opt in options {
        let id = opt.id.clone();
        let active = opt.id == selected_id;
        let disabled = opt.disabled;
        let on_select = on_select.clone();
        menu = menu.child(
            div()
                .px_3()
                .py_1p5()
                .text_xs()
                .text_color(if disabled {
                    theme.text_disabled
                } else if active {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .bg(if active {
                    theme.bg_selected
                } else {
                    theme.bg_elevated
                })
                .hover(|s| {
                    s.bg(if active {
                        theme.bg_selected
                    } else {
                        theme.bg_hover
                    })
                })
                .cursor_pointer()
                .when(!disabled, |el| {
                    el.on_mouse_down(MouseButton::Left, {
                        let id = id.clone();
                        cx.listener(move |_this, _, window, cx| {
                            cx.stop_propagation();
                            let id = id.clone();
                            let on_select = on_select.clone();
                            cx.defer_in(window, move |this, _, cx| {
                                on_select(this, id, cx);
                            });
                        })
                    })
                })
                .child(opt.label.clone()),
        );
    }

    menu
}

pub fn render_project_remove_row(
    path: String,
    anchor: Point<Pixels>,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([deepseekcode_i18n::t("sidebar.remove_from_sidebar")]);
    div()
        .id("popover-project-menu")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(anchor.y + px(4.))
        .w(width)
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .child(
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.error)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        let path = path.clone();
                        cx.defer_in(window, move |this, _, cx| {
                            this.close_project_context_menu(cx);
                            this.remove_project_from_sidebar(path, cx);
                        });
                    }),
                )
                .child(deepseekcode_i18n::t("sidebar.remove_from_sidebar")),
        )
}

pub fn render_session_delete_row(
    session_id: deepseekcode_types::SessionId,
    anchor: Point<Pixels>,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([deepseekcode_i18n::t("sidebar.delete_session")]);
    div()
        .id("popover-context-menu")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(anchor.y + px(4.))
        .w(width)
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .child(
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.text_primary)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        cx.defer_in(window, move |this, _, cx| {
                            this.delete_session_by_id(session_id, cx);
                        });
                    }),
                )
                .child(deepseekcode_i18n::t("sidebar.delete_session")),
        )
}

#[allow(dead_code)]
pub fn render_context_row(
    label: &'static str,
    anchor: Point<Pixels>,
    action: impl Fn(&mut crate::workspace::WorkspaceView, &mut Context<crate::workspace::WorkspaceView>)
        + 'static
        + Clone,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([label]);
    div()
        .id("popover-context-menu")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(anchor.y + px(4.))
        .w(width)
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .child(
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.text_primary)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        let action = action.clone();
                        cx.defer_in(window, move |this, _, cx| {
                            action(this, cx);
                        });
                    }),
                )
                .child(label),
        )
}

pub fn render_dismiss_layer(cx: &mut Context<crate::workspace::WorkspaceView>) -> impl IntoElement {
    div()
        .id("popover-dismiss")
        .absolute()
        .inset_0()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_this, _, window, cx| {
                cx.defer_in(window, |this, _, cx| {
                    this.close_popover(cx);
                });
            }),
        )
}
