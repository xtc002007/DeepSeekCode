//! Workspace-level popover menus — opaque, topmost, auto-width.

use gpui::prelude::FluentBuilder;
use gpui::*;

use super::dropdown::{DropDirection, DropdownOption};

pub fn menu_width_for_labels(labels: impl IntoIterator<Item = impl AsRef<str>>) -> Pixels {
    // P2 CJK: Chinese/emoji glyphs are ~2x wider than Latin. Count CJK-ish
    // chars at 14px and others at 7.2px so localized menus don't wrap/clip.
    let max_width_px = labels
        .into_iter()
        .map(|s| {
            let s = s.as_ref();
            let mut w = 0.0f32;
            for ch in s.chars() {
                let wide = matches!(ch,
                    '\u{2E80}'..='\u{9FFF}'
                    | '\u{AC00}'..='\u{D7AF}'
                    | '\u{F900}'..='\u{FAFF}'
                    | '\u{FF00}'..='\u{FFEF}'
                    | '\u{1F000}'..='\u{1FAFF}'
                    | '\u{2600}'..='\u{27BF}');
                w += if wide { 14.0 } else { 7.2 };
            }
            w
        })
        .fold(0.0f32, f32::max);
    px((max_width_px + 28.0).clamp(120.0, 420.0))
}

pub fn menu_width_for_options(options: &[DropdownOption]) -> Pixels {
    menu_width_for_labels(options.iter().map(|o| o.label.as_str()))
}

pub fn menu_top_for_direction(
    anchor: Point<Pixels>,
    direction: DropDirection,
    item_count: usize,
) -> Pixels {
    // P2: real rows are py_1p5/py_2 text_xs (~30-32px), not 28px. Use 32px so
    // Up menus don't overlap the anchor, and clamp both edges.
    let row_h = px(32.);
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
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
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
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .shadow_lg();

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
                    if disabled {
                        s
                    } else if active {
                        s.bg(theme.bg_selected)
                    } else {
                        s.bg(theme.bg_hover)
                    }
                })
                .cursor(if disabled {
                    CursorStyle::Arrow
                } else {
                    CursorStyle::PointingHand
                })
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
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([code_plus_plus_i18n::t("sidebar.remove_from_sidebar")]);
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
                .hover(|s| s.bg(theme.bg_hover))
                .hover(|s| s.bg(theme.bg_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        let path = path.clone();
                        cx.defer_in(window, move |this, _, cx| {
                            // P0-6: arm confirm, don't remove immediately.
                            // Keep popover_anchor for the confirm dialog.
                            this.project_context_menu = None;
                            this.request_remove_project(path, cx);
                        });
                    }),
                )
                .child(code_plus_plus_i18n::t("sidebar.remove_from_sidebar")),
        )
}

pub fn render_session_delete_row(
    session_id: code_plus_plus_types::SessionId,
    anchor: Point<Pixels>,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([code_plus_plus_i18n::t("sidebar.delete_session")]);
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
                            // P0-6: arm confirm, don't delete immediately.
                            this.session_context_menu = None;
                            this.request_delete_session(session_id, cx);
                        });
                    }),
                )
                .child(code_plus_plus_i18n::t("sidebar.delete_session")),
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
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
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
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_this, _, window, cx| {
                // P1-2: swallow the click so closing a popover never also
                // activates the tab/button underneath.
                cx.stop_propagation();
                cx.defer_in(window, |this, _, cx| {
                    this.close_popover(cx);
                });
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|_this, _, window, cx| {
                cx.stop_propagation();
                cx.defer_in(window, |this, _, cx| {
                    this.close_popover(cx);
                });
            }),
        )
}

pub fn render_session_delete_confirm(
    session_id: code_plus_plus_types::SessionId,
    anchor: Point<Pixels>,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([
        code_plus_plus_i18n::t("sidebar.confirm_delete_session"),
        code_plus_plus_i18n::t("common.confirm"),
    ]);
    // P2: clamp to viewport so the confirm never overflows the right edge.
    let left = anchor.x;
    div()
        .id("popover-delete-confirm")
        .occlude()
        .absolute()
        .left(left)
        .top(anchor.y + px(4.))
        .w(width)
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .child(
            div()
                .text_xs()
                .text_color(theme.text_primary)
                .child(code_plus_plus_i18n::t("sidebar.confirm_delete_session")),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .justify_end()
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, window, cx| {
                                cx.stop_propagation();
                                cx.defer_in(window, |this, _, cx| {
                                    this.cancel_pending_delete(cx);
                                });
                            }),
                        )
                        .child(code_plus_plus_i18n::t("common.cancel")),
                )
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.error)
                        .text_xs()
                        .text_color(theme.text_on_accent)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.error))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, window, cx| {
                                cx.stop_propagation();
                                cx.defer_in(window, move |this, _, cx| {
                                    this.delete_session_by_id(session_id, cx);
                                });
                            }),
                        )
                        .child(code_plus_plus_i18n::t("common.confirm")),
                ),
        )
}

pub fn render_project_remove_confirm(
    _path: String,
    anchor: Point<Pixels>,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    let width = menu_width_for_labels([
        code_plus_plus_i18n::t("sidebar.confirm_remove_project"),
        code_plus_plus_i18n::t("common.confirm"),
    ]);
    div()
        .id("popover-remove-confirm")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(anchor.y + px(4.))
        .w(width)
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .child(
            div()
                .text_xs()
                .text_color(theme.text_primary)
                .child(code_plus_plus_i18n::t("sidebar.confirm_remove_project")),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .justify_end()
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, window, cx| {
                                cx.stop_propagation();
                                cx.defer_in(window, |this, _, cx| {
                                    this.cancel_pending_delete(cx);
                                });
                            }),
                        )
                        .child(code_plus_plus_i18n::t("common.cancel")),
                )
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.error)
                        .text_xs()
                        .text_color(theme.text_on_accent)
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, window, cx| {
                                cx.stop_propagation();
                                cx.defer_in(window, |this, _, cx| {
                                    this.confirm_remove_project(cx);
                                });
                            }),
                        )
                        .child(code_plus_plus_i18n::t("common.confirm")),
                ),
        )
}
