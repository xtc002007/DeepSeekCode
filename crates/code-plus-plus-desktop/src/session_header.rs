//! Session header above the timeline in the center column.

use gpui::prelude::FluentBuilder;
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

        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);

        let breadcrumb = project_path
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| code_plus_plus_i18n::t("session.project"));

        let title = if has_session {
            session_title.to_string()
        } else {
            code_plus_plus_i18n::t("session.select_or_create")
        };

        // P2: status colors keyed on raw (English) state ids — display text is
        // localized via localize_session_status, so match the raw id here.
        let status_color = if session_busy {
            theme.success
        } else if session_status == "Ready" {
            theme.text_muted
        } else if session_status == "Error"
            || session_status == "Interrupted"
            || session_status.starts_with("Error")
        {
            theme.error
        } else if session_status.contains("Loading")
            || session_status.contains("Compacting")
            || session_status.contains("Waiting")
            || session_status.contains("Running")
            || session_status.contains("Thinking")
            || session_status.contains("Generating")
        {
            theme.warning
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
                    .child(div().text_xs().text_color(theme.text_disabled).child(
                        code_plus_plus_i18n::tr!(
                            "session.breadcrumb",
                            "breadcrumb" => breadcrumb.as_str()
                        ),
                    ))
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
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.save_session_title(cx);
                                        }),
                                    )
                                    .child(code_plus_plus_i18n::t("common.save")),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(theme.text_muted)
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.cancel_session_title_edit(cx);
                                        }),
                                    )
                                    .child(code_plus_plus_i18n::t("common.cancel")),
                            )
                            .into_any_element()]
                    } else {
                        vec![div()
                            .id("session-title")
                            .text_sm()
                            .line_height(crate::theme::leading_sm(&theme))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .overflow_hidden()
                            .text_ellipsis()
                            // P2 SH3: no pointer affordance when there's no
                            // session (previously a hand cursor that did nothing).
                            .when(has_session, |el| {
                                el.cursor_pointer().tooltip(move |_, cx| {
                                    cx.new(|_| crate::git_panel::GitFileTooltip {
                                        text: code_plus_plus_i18n::t("session.title_placeholder")
                                            .into(),
                                    })
                                    .into()
                                })
                            })
                            .on_mouse_up(MouseButton::Left, {
                                let has_session = has_session;
                                cx.listener(move |this, _, window, cx| {
                                    if has_session {
                                        cx.stop_propagation();
                                        this.start_session_title_edit(window, cx);
                                    }
                                })
                            })
                            .child(title)
                            .into_any_element()]
                    }),
            )
            .children(if has_session {
                let mut status_row = div().flex().items_center().gap_2();
                // P2 SH4: always render the plans entry so the feature is
                // discoverable at count==0 (previously the entry vanished).
                {
                    let label = code_plus_plus_i18n::tr!(
                        "session.plans_count",
                        "count" => plan_count as u64
                    );
                    status_row = status_row.child(
                        div()
                            .id("session-plans-pill")
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
                            .text_color(if plan_count > 0 {
                                theme.text_primary
                            } else {
                                theme.text_muted
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::git_panel::GitFileTooltip {
                                    text: code_plus_plus_i18n::t("plan.ready_title").into(),
                                })
                                .into()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_session_plans_menu(cx);
                                }),
                            )
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
                                .child(crate::status_text::localize_session_status(session_status)),
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

    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
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
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.new"),
            |this, cx| this.new_session(cx),
            &theme,
        ))
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.fork"),
            |this, cx| this.fork_current_session(cx),
            &theme,
        ))
        .children(if has_selected_message {
            vec![menu_item(
                cx,
                code_plus_plus_i18n::t("session.fork_from_message"),
                |this, cx| {
                    this.fork_current_session_from_message(cx);
                },
                &theme,
            )
            .into_any_element()]
        } else {
            vec![]
        })
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.export"),
            |this, cx| {
                this.export_current_session(cx);
            },
            &theme,
        ))
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.revert"),
            |this, cx| this.open_revert_dock(cx),
            &theme,
        ))
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.compact_now"),
            |this, cx| {
                this.compact_current_session(cx);
            },
            &theme,
        ))
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.move"),
            |this, cx| {
                this.move_current_session(cx);
            },
            &theme,
        ))
        .child(menu_item(
            cx,
            code_plus_plus_i18n::t("session.menu"),
            |this, cx| {
                this.open_settings(crate::settings::SettingsSection::Ai, cx);
            },
            &theme,
        ))
}

fn menu_item(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: String,
    action: fn(&mut crate::workspace::WorkspaceView, &mut Context<crate::workspace::WorkspaceView>),
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
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
                // P1-2: stop + defer so the action never races the dismiss
                // layer underneath (which would double-notify / flicker).
                cx.stop_propagation();
                cx.defer_in(window, move |this, _, cx| {
                    this.close_session_menu(cx);
                    action(this, cx);
                });
            }),
        )
        .child(label)
}
