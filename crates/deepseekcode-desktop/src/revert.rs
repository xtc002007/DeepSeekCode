//! Session revert dock — stage / clear / commit UI.

use deepseekcode_types::MessageId;
use gpui::*;

#[derive(Clone, Debug, Default)]
pub struct RevertDockState {
    pub open: bool,
    pub staged_message_id: Option<String>,
    pub diff_preview: String,
    pub warning: Option<String>,
    pub error: Option<String>,
}

pub struct RevertDock;

impl RevertDock {
    pub fn render_overlay(
        cx: &Context<crate::workspace::WorkspaceView>,
        state: &RevertDockState,
        selected_message_id: Option<MessageId>,
    ) -> AnyElement {
        if !state.open {
            return div().into_any_element();
        }

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let has_staged = state.staged_message_id.is_some();
        let stage_enabled = selected_message_id.is_some() && !has_staged;
        let commit_enabled = has_staged;
        let clear_enabled = has_staged;
        let status_line = if let Some(error) = &state.error {
            error.clone()
        } else if let Some(warning) = &state.warning {
            warning.clone()
        } else if let Some(staged) = &state.staged_message_id {
            format!("Staged at {staged} — review diff then commit or clear.")
        } else if let Some(id) = selected_message_id {
            format!("Selected {id} — click Stage to preview file changes.")
        } else {
            "Select a user or assistant message in the timeline, then Stage.".to_string()
        };

        div()
            .id("revert-dock")
            .absolute()
            .bottom(px(80.))
            .right(px(16.))
            .w(px(360.))
            .p_3()
            .rounded_lg()
            .bg(theme.bg_surface)
            .border_1()
            .border_color(theme.border_default)
            .flex()
            .flex_col()
            .gap_2()
            .shadow_lg()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child("Revert changes"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if state.error.is_some() {
                        theme.error
                    } else if state.warning.is_some() {
                        theme.warning
                    } else if stage_enabled || has_staged {
                        theme.text_secondary
                    } else {
                        theme.warning
                    })
                    .child(status_line),
            )
            .child(
                div()
                    .max_h(px(120.))
                    .overflow_hidden()
                    .text_xs()
                    .font_family("Consolas")
                    .text_color(theme.text_secondary)
                    .child(if state.diff_preview.is_empty() {
                        "No diff preview".to_string()
                    } else {
                        state.diff_preview.clone()
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(revert_button(
                        cx,
                        "Stage",
                        RevertAction::Stage,
                        stage_enabled,
                        &theme,
                    ))
                    .child(revert_button(
                        cx,
                        "Clear",
                        RevertAction::Clear,
                        clear_enabled,
                        &theme,
                    ))
                    .child(revert_button(
                        cx,
                        "Commit",
                        RevertAction::Commit,
                        commit_enabled,
                        &theme,
                    ))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_revert_dock(cx);
                                }),
                            )
                            .child("Close"),
                    ),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertAction {
    Stage,
    Clear,
    Commit,
}

fn revert_button(
    cx: &Context<crate::workspace::WorkspaceView>,
    label: &'static str,
    action: RevertAction,
    enabled: bool,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let mut button = div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(if enabled {
            theme.bg_elevated
        } else {
            theme.bg_base
        })
        .text_xs()
        .text_color(if enabled {
            theme.text_primary
        } else {
            theme.text_disabled
        });

    if enabled {
        button = button
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.handle_revert_action(action, cx);
                }),
            );
    }

    button.child(label)
}
