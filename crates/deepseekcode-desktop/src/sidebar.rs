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
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.toggle_sidebar_collapsed(cx);
                    })),
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
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.pick_project_folder(cx);
                    })),
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
                    .child("RECENT"),
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
                let is_current = current_project
                    .is_some_and(|current| crate::workspace::project_paths_equal(current, &nav.path));
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
                    .py_1p5()
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
                            .child("PROJECTS"),
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
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.toggle_sidebar_collapsed(cx);
                        })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1p5()
                    .flex_shrink_0()
                    .child(
                        button_shell(
                            "open-folder",
                            "Open Folder…",
                            &theme,
                            ButtonVariant::Secondary,
                            ButtonSize::Md,
                            false,
                            Some(IconName::FolderOpen),
                            false,
                        )
                        .w_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.pick_project_folder(cx);
                        })),
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
                    .flex_shrink_0()
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, {
                        let path_for_toggle = path_for_toggle.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.toggle_project_expanded(path_for_toggle.clone(), cx);
                            cx.stop_propagation();
                        })
                    })
                    .child(render_icon_rgba(chevron, px(12.), theme.text_muted)),
            )
            .child(
                div()
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
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, {
                        let path_for_open = path_for_open.clone();
                        let is_current = is_current;
                        cx.listener(move |this, _, _, cx| {
                            if is_current {
                                this.toggle_project_expanded(path_for_open.clone(), cx);
                            } else {
                                this.open_recent_project(PathBuf::from(path_for_open.clone()), cx);
                            }
                            cx.stop_propagation();
                        })
                    })
                    .child(short_name.to_string()),
            )
            .children(if show_sessions {
                vec![div()
                    .flex_shrink_0()
                    .px_1p5()
                    .py_0p5()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(|s| s.bg(sidebar_row_hover_bg(&theme)).text_color(theme.text_primary))
                    .on_mouse_down(MouseButton::Left, {
                        cx.listener(move |this, _, _, cx| {
                            this.new_session_for_project(
                                PathBuf::from(path_for_new.clone()),
                                cx,
                            );
                            cx.stop_propagation();
                        })
                    })
                    .child(render_icon_rgba(IconName::Plus, px(12.), theme.text_muted))
                    .into_any_element()]
            } else {
                vec![]
            });
        row = row.on_mouse_up(MouseButton::Right, {
            cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                this.open_project_context_menu(path_for_context.clone(), event.position, cx);
                cx.stop_propagation();
            })
        });
        row
    });

    if expanded && show_sessions {
        if loading_sessions && sessions.is_empty() {
            column = column.child(
                div()
                    .pl_6()
                    .pr_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.text_disabled)
                    .child("Loading…"),
            );
        } else if sessions.is_empty() {
            column = column.child(
                div()
                    .pl_6()
                    .pr_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.text_disabled)
                    .child("No sessions"),
            );
        } else {
            let limit = session_nav_visible
                .get(path)
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
                    format!("更多 ({remaining})")
                } else {
                    "更多".to_string()
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

    let mut row = div()
        .pl_6()
        .pr_2()
        .py_1()
        .mx_1()
        .rounded_md()
        .cursor_pointer()
        .min_h(px(28.))
        .flex()
        .items_center()
        .when(active, |s| s.bg(sidebar_session_selected_bg(&theme)));
    row.on_mouse_down(MouseButton::Left, {
            let project_path = project_path.clone();
            cx.listener(move |this, _, _, cx| {
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
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(if running {
                    theme.success
                } else {
                    theme.text_disabled
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
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
                .overflow_hidden()
                .child(title),
        )
}
