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
        inflight: bool,
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
                div().flex().items_center().justify_between().child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(deepseekcode_i18n::tr!(
                            "policy.title",
                            "scope" => scope.as_str(),
                            "tool" => tool.as_str(),
                            "queue_label" => queue_label.as_str()
                        )),
                )
                // P2: FIFO note so queued-behind is explicit (previously
                // later prompts were invisible with no explanation).
                .when(prompt.queue_total > 1, |el| {
                    el.child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.text_muted)
                            .child(format!(
                                "+{} queued (FIFO)",
                                prompt.queue_total - prompt.queue_index - 1
                            )),
                    )
                }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(deepseekcode_i18n::t("policy.body")),
            )
            // Bounded internal scroll: an uncapped detail (e.g. a full
            // todowrite JSON payload) grows the flex_shrink_0 dock without
            // limit and squeezes the composer out of the clipped column.
            // Same max_h + overflow_y_scroll pattern as the todo strip.
            .child(
                div()
                    .id("permission-dock-detail")
                    .rounded_md()
                    .bg(theme.bg_base)
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .child(crate::ui::multiline_text::render_multiline(&detail, |el| {
                        el.p_2()
                            .text_xs()
                            .font_family(crate::theme::font_mono(&theme))
                            .text_color(theme.text_primary)
                    })),
            )
            .when(!patterns.is_empty(), |el| {
                let count = patterns.len() as u64;
                let toggle_label = if patterns_expanded {
                    deepseekcode_i18n::tr!("policy.hide_patterns", "count" => count)
                } else {
                    deepseekcode_i18n::tr!("policy.show_patterns", "count" => count)
                };
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_policy_patterns_expanded(cx);
                            }),
                        )
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
                                    .font_family(crate::theme::font_mono(&theme))
                                    .text_color(theme.text_muted)
                                    .child(p.clone())
                            })),
                    )
                })
            })
            .when(reject_expanded, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(if inflight {
                                    deepseekcode_i18n::t("policy.sending")
                                } else {
                                    deepseekcode_i18n::t("policy.feedback")
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.cancel_policy_reject(cx);
                                    }),
                                )
                                .child(deepseekcode_i18n::t("common.cancel")),
                        ),
                )
                .child(
                    div()
                        .min_h(px(48.))
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
                            .id(ElementId::Name(format!("policy-deny-{id}").into()))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(if inflight {
                                theme.text_disabled
                            } else {
                                theme.error
                            })
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .when(!inflight, |el| el.cursor_pointer())
                            .when(inflight, |el| el.opacity(0.6))
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::git_panel::GitFileTooltip {
                                    text: if reject_expanded {
                                        deepseekcode_i18n::t("policy.deny_confirm").into()
                                    } else {
                                        deepseekcode_i18n::t("policy.deny_expand").into()
                                    },
                                })
                                .into()
                            })
                            .when(!inflight, |el| {
                                el.on_mouse_down(MouseButton::Left, {
                                    let id = id.clone();
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.handle_policy_deny_click(id.clone(), window, cx);
                                    })
                                })
                            })
                            .child(if reject_expanded {
                                deepseekcode_i18n::t("policy.deny_confirm")
                            } else {
                                deepseekcode_i18n::t("common.deny")
                            }),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("policy-always-{id}").into()))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.bg_surface)
                            .border_1()
                            .border_color(theme.border_default)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .when(!inflight, |el| el.cursor_pointer())
                            .when(inflight, |el| el.opacity(0.6))
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::git_panel::GitFileTooltip {
                                    text: deepseekcode_i18n::t("policy.always_hint").into(),
                                })
                                .into()
                            })
                            .when(!inflight, |el| {
                                el.on_mouse_down(MouseButton::Left, {
                                    let id = id.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.respond_policy_project(id.clone(), cx);
                                    })
                                })
                            })
                            .child(deepseekcode_i18n::t("common.always_project")),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("policy-allow-{id}").into()))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            // P2: both branches Rgba (accent is Hsla and would
                            // mismatch text_disabled). accent_muted keeps the
                            // blue family with the right type.
                            .bg(if inflight {
                                theme.text_disabled
                            } else {
                                theme.accent_muted
                            })
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .when(!inflight, |el| el.cursor_pointer())
                            .when(inflight, |el| el.opacity(0.6))
                            .tooltip(move |_, cx| {
                                cx.new(|_| crate::git_panel::GitFileTooltip {
                                    text: deepseekcode_i18n::t("policy.allow_once_hint").into(),
                                })
                                .into()
                            })
                            .when(!inflight, |el| {
                                el.on_mouse_down(MouseButton::Left, {
                                    let id = id.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.respond_policy_once(id.clone(), cx);
                                    })
                                })
                            })
                            .child(deepseekcode_i18n::t("common.allow_once")),
                    ),
            )
    }
}
