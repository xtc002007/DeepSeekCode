//! Collapsed plan session strip above composer — persistent plan access.

use code_plus_plus_schema::plan::SessionPlanSummary;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub struct PlanSessionStrip;

impl PlanSessionStrip {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        plans: &[SessionPlanSummary],
        expanded: bool,
        agent_is_plan: bool,
    ) -> Option<impl IntoElement> {
        if plans.is_empty() {
            return None;
        }
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
        let count = plans.len();
        let label = code_plus_plus_i18n::tr!("plan.ready_count", "count" => count as u64);
        let muted = !agent_is_plan;

        Some(
            div()
                .id("plan-session-strip")
                .w_full()
                .px_4()
                .py_2()
                .bg(if muted {
                    theme.bg_base
                } else {
                    theme.bg_elevated
                })
                .border_1()
                .border_color(theme.border_subtle)
                .rounded_md()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_plan_picker(cx);
                            }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.text_primary)
                                .child(label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(if expanded {
                                    code_plus_plus_i18n::t("common.hide")
                                } else {
                                    code_plus_plus_i18n::t("common.show")
                                }),
                        ),
                )
                .when(expanded, |el| {
                    el.children(plans.iter().map(|plan| {
                        let plan_id = plan.plan_id.clone();
                        let plan_path = plan.plan_path.clone();
                        let title = plan.title.clone();
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div().text_xs().text_color(theme.text_primary).child(title),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(crate::theme::font_mono(&theme))
                                            .text_color(theme.text_muted)
                                            .child(plan_path.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(strip_button(
                                        cx,
                                        code_plus_plus_i18n::t("plan.view_short"),
                                        {
                                            let plan_path = plan_path.clone();
                                            move |this, _, _, cx| {
                                                this.open_plan_file(&plan_path, cx)
                                            }
                                        },
                                    ))
                                    .child(strip_button(
                                        cx,
                                        code_plus_plus_i18n::t("plan.implement_short"),
                                        {
                                            let plan_id = plan_id.clone();
                                            move |this, _, _, cx| {
                                                this.implement_from_plan(&plan_id, cx)
                                            }
                                        },
                                    )),
                            )
                    }))
                }),
        )
    }
}

fn strip_button(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: String,
    handler: impl Fn(
            &mut crate::workspace::WorkspaceView,
            &MouseUpEvent,
            &mut Window,
            &mut Context<crate::workspace::WorkspaceView>,
        ) + 'static,
) -> impl IntoElement {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    let arm_id = format!("plan-strip-{label}");
    let confirm_id = arm_id.clone();
    let tooltip_label = label.clone();
    let display_label = label.clone();
    let tip_id = format!("plan-strip-tip-{display_label}");
    div()
        .id(ElementId::Name(tip_id.into()))
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(theme.bg_base)
        .text_xs()
        .text_color(theme.text_primary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .tooltip(move |_, cx| {
            cx.new(|_| crate::git_panel::GitFileTooltip {
                text: tooltip_label.clone().into(),
            })
            .into()
        })
        // P1-1: arm on down, fire on up (same row) — drag-through never
        // mis-fires Implement. P1-2: stop so the click never reaches the
        // strip header toggle underneath.
        .on_mouse_down(MouseButton::Left, {
            let arm_id = arm_id.clone();
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.arm_click(arm_id.clone(), cx);
            })
        })
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, ev, window, cx| {
                cx.stop_propagation();
                if !this.confirm_click(&confirm_id, cx) {
                    return;
                }
                handler(this, ev, window, cx);
            }),
        )
        .child(display_label)
}
