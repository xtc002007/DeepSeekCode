//! Composer plan implement dock — shown in plan mode when a plan completes.

use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Debug)]
pub struct PlanImplementPromptState {
    pub plan_id: String,
    pub title: String,
    pub plan_path: String,
    pub summary: Option<String>,
}

pub struct PlanImplementDock;

impl PlanImplementDock {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        prompt: &PlanImplementPromptState,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let plan_id = prompt.plan_id.clone();
        let plan_path = prompt.plan_path.clone();
        let title = prompt.title.clone();
        let summary = prompt.summary.clone();

        div()
            .id("plan-implement-dock")
            .w_full()
            .px_4()
            .py_3()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.info)
            .rounded_lg()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child(format!("Plan ready · {title}")),
            )
            .when(summary.is_some(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(summary.clone().unwrap_or_default()),
                )
            })
            .child(
                div()
                    .text_xs()
                    .font_family("Consolas")
                    .text_color(theme.text_muted)
                    .child(plan_path.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(dock_button(
                        cx,
                        "View plan",
                        theme.bg_base,
                        theme.text_primary,
                        {
                            let plan_path = plan_path.clone();
                            move |this, _, _, cx| this.open_plan_file(&plan_path, cx)
                        },
                    ))
                    .child(dock_button(
                        cx,
                        "Implement plan",
                        theme.info,
                        theme.text_on_accent,
                        {
                            let plan_id = plan_id.clone();
                            move |this, _, _, cx| this.implement_from_plan(&plan_id, cx)
                        },
                    ))
                    .child(dock_button(
                        cx,
                        "Dismiss",
                        theme.bg_base,
                        theme.text_secondary,
                        {
                            let plan_id = plan_id.clone();
                            move |this, _, _, cx| this.dismiss_plan_implement_prompt(&plan_id, cx)
                        },
                    )),
            )
    }
}

fn dock_button(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: &'static str,
    bg: gpui::Rgba,
    fg: gpui::Rgba,
    handler: impl Fn(
            &mut crate::workspace::WorkspaceView,
            &MouseDownEvent,
            &mut Window,
            &mut Context<crate::workspace::WorkspaceView>,
        ) + 'static,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(bg)
        .text_xs()
        .text_color(fg)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(handler))
        .child(label)
}
