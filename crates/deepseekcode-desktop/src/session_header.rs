//! Session header above the timeline in the center column.

use gpui::*;

pub struct SessionHeaderView;

impl SessionHeaderView {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        has_project: bool,
        has_session: bool,
        project_path: Option<&str>,
        session_title: &str,
        title_editing: bool,
        session_title_input: Entity<crate::composer::PromptInput>,
        session_busy: bool,
        session_status: &str,
        plan_count: usize,
        plans_menu_open: bool,
    ) -> impl IntoElement {
        if !has_project {
            return div().into_any_element();
        }

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        let breadcrumb = project_path
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Project");

        let title = if has_session {
            session_title.to_string()
        } else {
            "Select or create a session".to_string()
        };

        let status_color = if session_busy {
            theme.success
        } else if session_status == "Ready" {
            theme.text_muted
        } else if session_status == "Error" || session_status == "Interrupted" {
            theme.error
        } else {
            theme.text_secondary
        };

        div()
            .id("session-header")
            .w_full()
            .flex_shrink_0()
            .px_4()
            .py_1p5()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.border_subtle)
            .bg(theme.bg_base)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w(px(0.))
                    .flex_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .child(format!("{breadcrumb} › Session")),
                    )
                    .children(if has_session && title_editing {
                        vec![div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(280.)).child(session_title_input.clone()))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(theme.accent_muted)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.save_session_title(cx);
                                    }))
                                    .child("Save"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(theme.text_muted)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.cancel_session_title_edit(cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .into_any_element()]
                    } else {
                        vec![div()
                            .text_sm()
                            .line_height(crate::theme::leading_sm(&theme))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let has_session = has_session;
                                cx.listener(move |this, _, _, cx| {
                                    if has_session {
                                        this.start_session_title_edit(cx);
                                    }
                                })
                            })
                            .child(title)
                            .into_any_element()]
                    }),
            )
            .children(if has_session {
                let mut status_row = div().flex().items_center().gap_2();
                if plan_count > 0 {
                    let label = format!("Plans ({plan_count})");
                    status_row = status_row.child(
                        div()
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if plans_menu_open {
                                theme.bg_selected
                            } else {
                                theme.bg_elevated
                            })
                            .border_1()
                            .border_color(theme.border_subtle)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.toggle_session_plans_menu(cx);
                            }))
                            .child(label),
                    );
                }
                status_row = status_row.child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(420.))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(if session_busy {
                            theme.accent_muted
                        } else {
                            theme.bg_elevated
                        })
                        .overflow_hidden()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(if session_busy {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(status_color)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(session_status.to_string()),
                        ),
                );
                vec![status_row.into_any_element()]
            } else {
                vec![]
            })
            .into_any_element()
    }
}

pub fn render_session_menu_panel(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    anchor: Point<Pixels>,
    has_selected_message: bool,
) -> impl IntoElement {
    use crate::ui::dropdown::DropDirection;
    use crate::ui::popover::menu_top_for_direction;

    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let item_count = if has_selected_message { 8 } else { 7 };
    let top = menu_top_for_direction(anchor, DropDirection::Up, item_count);
    div()
        .id("session-menu-panel")
        .occlude()
        .absolute()
        .left(anchor.x)
        .top(top)
        .w(px(200.))
        .flex()
        .flex_col()
        .py_1()
        .rounded_md()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_default)
        .shadow_lg()
        .child(menu_item(cx, "New Session", |this, cx| this.new_session(cx), &theme))
        .child(menu_item(
            cx,
            "Fork Session",
            |this, cx| this.fork_current_session(cx),
            &theme,
        ))
        .children(if has_selected_message {
            vec![menu_item(cx, "Fork from message", |this, cx| {
                this.fork_current_session(cx);
            }, &theme)
            .into_any_element()]
        } else {
            vec![]
        })
        .child(menu_item(cx, "Export Session…", |this, cx| {
            this.export_current_session(cx);
        }, &theme))
        .child(menu_item(cx, "Revert…", |this, cx| this.open_revert_dock(cx), &theme))
        .child(menu_item(cx, "Compact Now", |this, cx| {
            this.compact_current_session(cx);
        }, &theme))
        .child(menu_item(cx, "Move session…", |this, cx| {
            this.move_current_session(cx);
        }, &theme))
        .child(menu_item(cx, "Session Settings", |this, cx| {
            this.open_settings(crate::settings::SettingsSection::Ai, cx);
        }, &theme))
}

fn menu_item(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: &'static str,
    action: fn(&mut crate::workspace::WorkspaceView, &mut Context<crate::workspace::WorkspaceView>),
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .text_xs()
        .text_color(theme.text_primary)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.close_session_menu(cx);
            action(this, cx);
        }))
        .child(label)
}
