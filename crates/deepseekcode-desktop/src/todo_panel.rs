use deepseekcode_schema::todo::TodoItem;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub struct TodoPanel;

fn todo_status_icon(status: &str) -> IconName {
    match status {
        "completed" => IconName::Check,
        "in_progress" => IconName::ArrowRight,
        "cancelled" => IconName::X,
        _ => IconName::Circle,
    }
}

fn todo_status_color(status: &str, theme: &crate::theme::ThemeTokens) -> Rgba {
    match status {
        "completed" => theme.success,
        "in_progress" => theme.info,
        "cancelled" => theme.text_muted,
        _ => theme.text_secondary,
    }
}

fn next_todo_status(status: &str) -> &'static str {
    match status {
        "pending" => "in_progress",
        "in_progress" => "completed",
        "completed" => "pending",
        _ => "pending",
    }
}

fn todo_progress_label(todos: &[TodoItem]) -> String {
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let in_progress = todos.iter().filter(|t| t.status == "in_progress").count();
    let total = todos.len();
    if in_progress > 0 {
        format!("{completed}/{total} · {in_progress} in progress")
    } else {
        format!("{completed}/{total}")
    }
}

impl TodoPanel {
    /// Collapsible todo strip above the composer (layout flow, never overlays chat).
    pub fn render_composer_strip(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        todos: &[TodoItem],
        expanded: bool,
        highlighted_index: Option<usize>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let chevron = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        let summary = todo_progress_label(todos);

        let header = div()
            .id("composer-todo-header")
            .px_3()
            .py_1p5()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .bg(theme.bg_surface)
            .border_t_1()
            .border_b_1()
            .border_color(theme.border_default)
            .hover(|s| s.bg(theme.bg_hover))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.toggle_composer_todo_expanded(cx);
            }))
            .child(render_icon_rgba(chevron, px(12.), theme.text_muted))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child("Tasks"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(summary),
            );

        let mut strip = div()
            .id("composer-todo-strip")
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(header);

        if expanded {
            strip = strip.child(
                div()
                    .id("composer-todo-list")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .border_b_1()
                    .border_color(theme.border_default)
                    .bg(theme.bg_base)
                    .child(Self::render_list(cx, todos, highlighted_index)),
            );
        }

        strip
    }

    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        todos: &[TodoItem],
        highlighted_index: Option<usize>,
    ) -> impl IntoElement {
        Self::render_list(cx, todos, highlighted_index)
    }

    fn render_list(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        todos: &[TodoItem],
        highlighted_index: Option<usize>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let mut list = div().flex().flex_col().gap_1().p_2();
        if todos.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("No tasks yet."),
            );
        } else {
            for (idx, item) in todos.iter().enumerate() {
                let icon = todo_status_icon(&item.status);
                let icon_color = todo_status_color(&item.status, &theme);
                let next_status = next_todo_status(&item.status);
                let is_highlighted = highlighted_index == Some(idx);
                list = list.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(if is_highlighted {
                            theme.bg_selected
                        } else {
                            theme.bg_surface
                        })
                        .flex()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(MouseButton::Left, {
                            cx.listener(move |this, _, _, cx| {
                                this.highlight_todo_at_index(idx, cx);
                            })
                        })
                        .child(
                            div()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, {
                                    cx.listener(move |this, _, _, cx| {
                                        this.cycle_todo_status(idx, next_status, cx);
                                    })
                                })
                                .child(render_icon_rgba(icon, px(14.), icon_color)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(theme.text_primary)
                                .child(item.content.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_disabled)
                                .child(item.priority.clone()),
                        ),
                );
            }
        }
        list
    }
}
