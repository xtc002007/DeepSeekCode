//! Context menu helpers (re-export patterns for file/session menus).

use gpui::*;

/// Legacy single-row helper — kept for callers that build menus outside the
/// popover overlay. New code should prefer `popover::render_*_row` (which adds
/// `occlude + stop + defer`). This wrapper now matches that contract so the
/// two menu families can't diverge again (P2).
pub fn menu_row(
    label: &'static str,
    action: impl Fn(&mut crate::workspace::WorkspaceView, &mut Context<crate::workspace::WorkspaceView>)
        + 'static
        + Clone,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
    div()
        .px_3()
        .py_2()
        .text_xs()
        .text_color(theme.text_primary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .occlude()
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |_this, _, window, cx| {
                cx.stop_propagation();
                let action = action.clone();
                cx.defer_in(window, move |this, _, cx| {
                    action(this, cx);
                });
            }),
        )
        .child(label)
}
