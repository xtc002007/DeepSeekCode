//! Collapsed plan session strip above composer — persistent plan access.

use deepseekcode_schema::plan::SessionPlanSummary;
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
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let count = plans.len();
        let label = if count == 1 {
            "1 plan ready".to_string()
        } else {
            format!("{count} plans ready")
        };
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
                                .child(if expanded { "Hide" } else { "Show" }),
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
                                            .font_family("Consolas")
                                            .text_color(theme.text_muted)
                                            .child(plan_path.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(strip_button(cx, "View", {
                                        let plan_path = plan_path.clone();
                                        move |this, _, _, cx| this.open_plan_file(&plan_path, cx)
                                    }))
                                    .child(strip_button(cx, "Implement", {
                                        let plan_id = plan_id.clone();
                                        move |this, _, _, cx| this.implement_from_plan(&plan_id, cx)
                                    })),
                            )
                    }))
                }),
        )
    }
}

fn strip_button(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: &'static str,
    handler: impl Fn(
            &mut crate::workspace::WorkspaceView,
            &MouseDownEvent,
            &mut Window,
            &mut Context<crate::workspace::WorkspaceView>,
        ) + 'static,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(theme.bg_base)
        .text_xs()
        .text_color(theme.text_primary)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(handler))
        .child(label)
}
