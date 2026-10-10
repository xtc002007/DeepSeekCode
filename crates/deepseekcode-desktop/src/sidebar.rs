//! Left sidebar: projects and per-project session lists.

use crate::home::RecentProject;
use crate::layout::{ProjectNavItem, SessionNavItem};
use crate::theme::ThemeTokens;
use crate::ui::button::{button_shell, icon_button_shell, ButtonSize, ButtonVariant};
use crate::ui::icon::{render_icon_rgba, IconName};
use crate::ui::scrollbar::{render_vertical_scrollbar, SCROLLBAR_WIDTH};
use deepseekcode_types::SessionId;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;

/// Subtle pill background for the active session row (Cursor-like, dark-theme friendly).
fn sidebar_session_selected_bg(theme: &ThemeTokens) -> Rgba {
    theme.bg_active
}

fn sidebar_row_hover_bg(theme: &ThemeTokens) -> Rgba {
    theme.bg_hover
}

pub struct SidebarView;

impl SidebarView {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        collapsed: bool,
        width: Pixels,
        has_project: bool,
        current_project: Option<&str>,
        projects: &[ProjectNavItem],
        recent_projects: &[RecentProject],
        active_session: Option<SessionId>,
        expanded_projects: &std::collections::HashSet<String>,
        session_nav_visible: &std::collections::HashMap<String, usize>,
        project_sessions_loading: &std::collections::HashSet<String>,
        scroll_handle: &ScrollHandle,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        if collapsed {
            return div()
                .id("sidebar-collapsed")
                .w(px(40.))
                .h_full()
                .flex_shrink_0()
                .bg(theme.bg_surface)
                .border_r_1()
                .border_color(theme.border_subtle)
                .flex()
                .flex_col()
                .items_center()
                .py_2()
                .gap_2()
                .child(
                    icon_button_shell(
                        "sidebar-expand",
                        IconName::PanelLeftOpen,
                        &theme,
                        px(28.),
                        false,
                        false,
                        false,
                    )
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::git_panel::GitFileTooltip {
                            text: deepseekcode_i18n::t("sidebar.expand").into(),
                        })
                        .into()
                    })
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_sidebar_collapsed(cx);
                        }),
                    ),
                )
                .child(
                    icon_button_shell(
                        "sidebar-add-project",
                        IconName::Plus,
                        &theme,
                        px(28.),
                        false,
                        false,
                        false,
                    )
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::git_panel::GitFileTooltip {
                            text: deepseekcode_i18n::t("home.open_folder").into(),
                        })
                        .into()
                    })
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.pick_project_folder(cx);
                        }),
                    ),
                );
        }

        let mut list = div().w_full().flex().flex_col().gap_1();

        if !has_project {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .line_height(crate::theme::leading_xs(&theme))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(deepseekcode_i18n::t("sidebar.recent")),
            );
            for project in recent_projects {
                let path = project.path.clone();
                list = list.child(render_project_row(
                    cx,
                    &theme,
                    &path,
                    false,
                    false,
                    &[],
                    active_session,
                    expanded_projects,
                    session_nav_visible,
                    project_sessions_loading,
                ));
            }
        } else {
            for nav in projects {
                let is_current = current_project.is_some_and(|current| {
                    crate::workspace::project_paths_equal(current, &nav.path)
                });
                list = list.child(render_project_row(
                    cx,
                    &theme,
                    &nav.path,
                    is_current,
                    true,
                    &nav.sessions,
                    active_session,
                    expanded_projects,
                    session_nav_visible,
                    project_sessions_loading,
                ));
            }
            if projects.is_empty() {
                if let Some(path) = current_project {
                    list = list.child(render_project_row(
                        cx,
                        &theme,
                        path,
                        true,
                        true,
                        &[],
                        active_session,
                        expanded_projects,
                        session_nav_visible,
                        project_sessions_loading,
                    ));
                }
            }
        }

        div()
            .id("sidebar")
            .w(width)
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.bg_surface)
            .border_r_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .px_3()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_xs()
                            .line_height(crate::theme::leading_xs(&theme))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(deepseekcode_i18n::t("sidebar.projects")),
                    )
                    .child(
                        icon_button_shell(
                            "sidebar-collapse",
                            IconName::PanelLeftClose,
                            &theme,
                            px(24.),
                            false,
                            false,
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_sidebar_collapsed(cx);
                            }),
                        ),
                    ),
            )
            .child(
                div().px_3().py_1p5().flex_shrink_0().child(
                    button_shell(
                        "open-folder",
                        &deepseekcode_i18n::t("home.open_folder"),
                        &theme,
                        ButtonVariant::Secondary,
                        ButtonSize::Md,
                        false,
                        Some(IconName::FolderOpen),
                        false,
                    )
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.pick_project_folder(cx);
                        }),
                    ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .child(
                        div()
                            .id("sidebar-scroll")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .right(SCROLLBAR_WIDTH)
                            .track_scroll(scroll_handle)
                            .overflow_y_scroll()
                            .child(list),
                    )
                    .child(render_vertical_scrollbar(
                        cx,
                        scroll_handle,
                        "sidebar-scrollbar-track",
                        false,
                    )),
            )
    }
}

fn render_project_row(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    path: &str,
    is_current: bool,
    show_sessions: bool,
    sessions: &[SessionNavItem],
    active_session: Option<SessionId>,
    expanded_projects: &std::collections::HashSet<String>,
    session_nav_visible: &std::collections::HashMap<String, usize>,
    project_sessions_loading: &std::collections::HashSet<String>,
) -> impl IntoElement {
    const PAGE: usize = 5;
    let path_owned = path.to_string();
    let path_key = crate::workspace::project_path_key(path);
    let expanded = expanded_projects.contains(&path_key);
    let loading_sessions = project_sessions_loading.contains(&path_key);
    let short_name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    let theme = theme.clone();
    let mut column = div().flex().flex_col();
    let chevron = if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    };

    column = column.child({
        let path_for_toggle = path_owned.clone();
        let path_for_open = path_owned.clone();
        let path_for_new = path_owned.clone();
        let path_for_context = path_owned.clone();
        let path_for_context_right = path_owned.clone();
        let tooltip_path = path_owned.clone();
        let mut row = div()
            .id(format!("project-header-{path_key}"))
            .px_2()
            .py_1()
            .mx_1()
            .rounded_md()
            .flex()
            .flex_row()
            .flex_nowrap()
            .items_center()
            .gap_1()
            .w_full();
        row = row
            .child(
                div()
                    .id(format!("project-chevron-{path_key}"))
                    .flex_shrink_0()
                    // P2 S5: 24px hit area (was 16px) + hover bg so the
                    // chevron is actually hittable on HiDPI/touch.
                    .w(px(24.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(sidebar_row_hover_bg(&theme)))
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::git_panel::GitFileTooltip {
                            text: if expanded {
                                deepseekcode_i18n::t("common.hide").into()
                            } else {
                                deepseekcode_i18n::t("common.show").into()
                            },
                        })
                        .into()
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let path_for_toggle = path_for_toggle.clone();
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_project_expanded(path_for_toggle.clone(), cx);
                        })
                    })
                    .child(render_icon_rgba(chevron, px(12.), theme.text_muted)),
            )
            .child(
                div()
                    .id(format!("project-name-{path_key}"))
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .line_height(crate::theme::leading_sm(&theme))
                    .text_color(if is_current {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .font_weight(if is_current {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .cursor_pointer()
                    .hover(|s| s.bg(sidebar_row_hover_bg(&theme)))
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::git_panel::GitFileTooltip {
                            text: tooltip_path.clone().into(),
                        })
                        .into()
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let path_for_open = path_for_open.clone();
                        let is_current = is_current;
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if is_current {
                                this.toggle_project_expanded(path_for_open.clone(), cx);
                            } else {
                                this.open_recent_project(PathBuf::from(path_for_open.clone()), cx);
                            }
                        })
                    })
                    .child(short_name.to_string()),
            )
            .children(if show_sessions {
                vec![div()
                    .id(format!("project-new-{path_key}"))
                    .flex_shrink_0()
                    // P2 S7: 20px hit area (was 12px icon only) + hover bg +
                    // tooltip so the new-session affordance is discoverable.
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(|s| {
                        s.bg(sidebar_row_hover_bg(&theme))
                            .text_color(theme.text_primary)
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::git_panel::GitFileTooltip {
                            text: deepseekcode_i18n::t("sidebar.new_session").into(),
                        })
                        .into()
                    })
                    .on_mouse_up(MouseButton::Left, {
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.new_session_for_project(PathBuf::from(path_for_new.clone()), cx);
                        })
                    })
                    .child(render_icon_rgba(IconName::Plus, px(12.), theme.text_muted))
                    .into_any_element()]
            } else {
                vec![]
            });
        row = row.on_mouse_up(MouseButton::Right, {
            cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                this.open_project_context_menu(path_for_context_right.clone(), event.position, cx);
                cx.stop_propagation();
            })
        });
        // P2: Ctrl+Click (macOS trackpad) opens the same context menu.
        row = row.on_mouse_down(MouseButton::Left, {
            let path_for_context = path_for_context.clone();
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.modifiers.control {
                    this.open_project_context_menu(
                        path_for_context.clone(),
                        event.position,
                        cx,
                    );
                    cx.stop_propagation();
                }
            })
        });
        row
    });

    // P2 S5: show the session list whenever expanded (was `expanded &&
    // show_sessions`, so recent-project chevrons appeared dead).
    if expanded {
        if loading_sessions && sessions.is_empty() {
            column = column.child(
                div()
                    .pl_6()
                    .pr_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.text_disabled)
                    .child(deepseekcode_i18n::t("common.loading")),
            );
        } else if sessions.is_empty() {
            column = column.child(
                div()
                    .pl_6()
                    .pr_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.text_disabled)
                    .child(deepseekcode_i18n::t("sidebar.no_sessions")),
            );
        } else {
            let limit = session_nav_visible
                .get(&path_key)
                .copied()
                .unwrap_or(PAGE)
                .min(sessions.len());
            for session in &sessions[..limit] {
                column = column.child(render_session_row(
                    cx,
                    &theme,
                    path,
                    session,
                    active_session,
                ));
            }
            if sessions.len() > limit {
                let path_owned = path_owned.clone();
                let remaining = sessions.len() - limit;
                let label = if remaining > PAGE {
                    deepseekcode_i18n::tr!("sidebar.more_count", "count" => remaining)
                } else {
                    deepseekcode_i18n::t("sidebar.more")
                };
                column = column.child(
                    div()
                        .pl_6()
                        .pr_2()
                        .py_1()
                        .mx_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(theme.info)
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(MouseButton::Left, {
                            cx.listener(move |this, _, _, cx| {
                                // P1-2: stop so More never bubbles to the
                                // project row toggle underneath.
                                cx.stop_propagation();
                                this.show_more_project_sessions(path_owned.clone(), cx);
                            })
                        })
                        .child(label),
                );
            }
        }
    }

    column
}

fn render_session_row(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    theme: &ThemeTokens,
    project_path: &str,
    session: &SessionNavItem,
    active_session: Option<SessionId>,
) -> impl IntoElement {
    let theme = theme.clone();
    let active = active_session == Some(session.id);
    let session_id = session.id;
    let title = session.title.clone();
    let running = session.running;
    let project_path = project_path.to_string();

    let full_title: SharedString = if title.is_empty() {
        deepseekcode_i18n::t("workspace.new_session_short").into()
    } else {
        title.clone().into()
    };
    // P2 S12: running is text as well as color (color-blind safe) and the
    // tooltip shows title + state (was a terminal-styled tooltip).
    let state_label: SharedString = if running {
        deepseekcode_i18n::t("status.running").into()
    } else {
        full_title.clone()
    };
    let row = div()
        .id(ElementId::Name(format!("session-row-{}", session_id).into()))
        .pl_6()
        .pr_2()
        .py_1()
        .mx_1()
        .rounded_md()
        .cursor_pointer()
        .min_h(px(28.))
        .flex()
        .items_center()
        .when(active, |s| s.bg(sidebar_session_selected_bg(&theme)))
        .tooltip(move |_, cx| {
            cx.new(|_| crate::git_panel::GitFileTooltip {
                text: state_label.clone(),
            })
            .into()
        });
    row.on_mouse_down(MouseButton::Left, {
        let row_id = format!("session-row-{session_id}");
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            // P1-1: arm only — fire on mouse-up so press-drag-release never
            // mis-switches sessions. P1-2: stop so the dismiss layer doesn't
            // double-fire underneath.
            // P2: macOS Ctrl+Click / trackpad two-finger maps to Right here.
            if event.modifiers.control {
                this.open_session_context_menu(session_id, event.position, cx);
                cx.stop_propagation();
                return;
            }
            cx.stop_propagation();
            this.arm_click(row_id.clone(), cx);
        })
    })
    .on_mouse_up(MouseButton::Left, {
        let project_path = project_path.clone();
        let row_id = format!("session-row-{session_id}");
        cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            if !this.confirm_click(&row_id, cx) {
                return;
            }
            this.close_popover(cx);
            this.activate_project_session(
                std::path::PathBuf::from(project_path.clone()),
                session_id,
                cx,
            );
        })
    })
    .on_mouse_up(MouseButton::Right, {
        cx.listener(move |this, event: &MouseUpEvent, _, cx| {
            this.open_session_context_menu(session_id, event.position, cx);
            cx.stop_propagation();
        })
    })
    .gap_1()
    .child(
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(if running {
                theme.success
            } else {
                theme.text_disabled
            }))
            // P2 S12: text badge in addition to the dot so running doesn't
            // rely on color alone.
            .when(running, |el| {
                el.child(
                    div()
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.success)
                        .child("●"),
                )
            }),
    )
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .child(
                div()
                    .text_xs()
                    .line_height(crate::theme::leading_sm(&theme))
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active {
                        theme.text_primary
                    } else {
                        theme.text_muted
                    })
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(if title.is_empty() {
                        deepseekcode_i18n::t("workspace.new_session_short")
                    } else {
                        title
                    }),
            ),
    )
}
