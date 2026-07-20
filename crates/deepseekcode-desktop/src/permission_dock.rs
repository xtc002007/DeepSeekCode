//! Composer permission dock — policy prompt UI above the prompt input.

use crate::composer::PromptInput;
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Debug)]
pub struct PolicyPromptState {
    pub id: String,
    pub scope: String,
    pub tool: String,
    pub detail: String,
    pub patterns: Vec<String>,
    pub queue_index: usize,
    pub queue_total: usize,
}

pub struct PermissionDock;

impl PermissionDock {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        prompt: &PolicyPromptState,
        reject_expanded: bool,
        patterns_expanded: bool,
        reject_input: Entity<PromptInput>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let id = prompt.id.clone();
        let scope = prompt.scope.clone();
        let tool = prompt.tool.clone();
        let detail = prompt.detail.clone();
        let patterns = prompt.patterns.clone();
        let queue_label = if prompt.queue_total > 1 {
            format!(" ({}/{})", prompt.queue_index + 1, prompt.queue_total)
        } else {
            String::new()
        };

        div()
            .id("permission-dock")
            .w_full()
            .px_4()
            .py_3()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.warning)
            .rounded_lg()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(format!("Policy required · {scope} · {tool}{queue_label}")),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child("Review this action before the agent continues."),
            )
            .child(
                crate::ui::multiline_text::render_multiline(
                    &detail,
                    |el| {
                        el.p_2()
                            .rounded_md()
                            .bg(theme.bg_base)
                            .text_xs()
                            .font_family("Consolas")
                            .text_color(theme.text_primary)
                    },
                ),
            )
            .when(!patterns.is_empty(), |el| {
                let count = patterns.len();
                let toggle_label = if patterns_expanded {
                    format!("Hide {count} pattern(s)")
                } else {
                    format!("Show {count} pattern(s)")
                };
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.toggle_policy_patterns_expanded(cx);
                        }))
                        .child(toggle_label),
                )
                .when(patterns_expanded, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(patterns.iter().map(|p| {
                                div()
                                    .text_xs()
                                    .font_family("Consolas")
                                    .text_color(theme.text_muted)
                                    .child(p.clone())
                            })),
                    )
                })
            })
            .when(reject_expanded, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child("Tell the agent what to do differently (optional):"),
                )
                .child(
                    div()
                        .min_h(px(36.))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.bg_base)
                        .child(reject_input.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.error)
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let id = id.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.handle_policy_deny_click(id.clone(), cx);
                                })
                            })
                            .child("Deny"),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.bg_surface)
                            .border_1()
                            .border_color(theme.border_default)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let id = id.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.respond_policy_project(id.clone(), cx);
                                })
                            })
                            .child("Always (project)"),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.accent)
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let id = id.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.respond_policy_once(id.clone(), cx);
                                })
                            })
                            .child("Allow once"),
                    ),
            )
    }
}
