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
                    .child(deepseekcode_i18n::tr!("plan.ready", "title" => title.as_str())),
            )
            .when(summary.is_some(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(summary.clone().unwrap_or_default()),
                )
            })
            // P2: placeholder when the model produced no summary (was a
            // missing row that shifted layout).
            .when(summary.is_none(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_disabled)
                        .child(deepseekcode_i18n::t("common.none")),
                )
            })
            .child(
                div()
                    .id(ElementId::Name(format!("plan-path-{}", prompt.plan_id).into()))
                    .text_xs()
                    .font_family(crate::theme::font_mono(&theme))
                    .text_color(theme.text_muted)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .tooltip({
                        let tip_path = prompt.plan_path.clone();
                        move |_, cx| {
                            cx.new(|_| crate::git_panel::GitFileTooltip {
                                text: tip_path.clone().into(),
                            })
                            .into()
                        }
                    })
                    .child(prompt.plan_path.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_end()
                    .child(dock_button(
                        cx,
                        deepseekcode_i18n::t("plan.view"),
                        theme.bg_base,
                        theme.text_primary,
                        {
                            let plan_path = plan_path.clone();
                            move |this, _, _, cx| this.open_plan_file(&plan_path, cx)
                        },
                    ))
                    .child(dock_button(
                        cx,
                        deepseekcode_i18n::t("plan.implement"),
                        theme.info,
                        theme.text_on_accent,
                        {
                            let plan_id = plan_id.clone();
                            move |this, _, _, cx| this.implement_from_plan(&plan_id, cx)
                        },
                    ))
                    .child(dock_button(
                        cx,
                        deepseekcode_i18n::t("common.dismiss"),
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
    label: String,
    bg: gpui::Rgba,
    fg: gpui::Rgba,
    handler: impl Fn(
            &mut crate::workspace::WorkspaceView,
            &MouseUpEvent,
            &mut Window,
            &mut Context<crate::workspace::WorkspaceView>,
        ) + 'static,
) -> impl IntoElement {
    let tip = label.clone();
    let btn_id = format!("plan-dock-{label}");
    div()
        .id(ElementId::Name(btn_id.into()))
        .px_3()
        .py_1()
        .rounded_md()
        .bg(bg)
        .text_xs()
        .text_color(fg)
        .cursor_pointer()
        .hover(|s| s.bg(bg))
        .tooltip(move |_, cx| {
            cx.new(|_| crate::git_panel::GitFileTooltip {
                text: tip.clone().into(),
            })
            .into()
        })
        // P1-1 release-to-confirm + P1-2 stop.
        .on_mouse_up(MouseButton::Left, cx.listener(move |this, ev, window, cx| {
            cx.stop_propagation();
            handler(this, ev, window, cx);
        }))
        .child(label)
}
