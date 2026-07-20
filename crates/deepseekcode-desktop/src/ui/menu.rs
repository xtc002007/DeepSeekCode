//! Context menu helpers (re-export patterns for file/session menus).

use gpui::*;

pub fn menu_row(
    label: &'static str,
    action: impl Fn(&mut crate::workspace::WorkspaceView, &mut Context<crate::workspace::WorkspaceView>)
        + 'static,
    cx: &mut Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .px_3()
        .py_2()
        .text_xs()
        .text_color(theme.text_primary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            action(this, cx);
        }))
        .child(label)
}
