//! Draggable panel width handles for the three-column shell.

use gpui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelResizeEdge {
    Sidebar,
    RightPanel,
}

struct ResizeDragGhost;

impl Render for ResizeDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub fn render_resize_handle(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    edge: PanelResizeEdge,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    div()
        .id(match edge {
            PanelResizeEdge::Sidebar => "resize-sidebar",
            PanelResizeEdge::RightPanel => "resize-right-panel",
        })
        .w(px(4.))
        .h_full()
        .flex_shrink_0()
        .cursor_col_resize()
        .bg(hsla(0., 0., 0., 0.))
        .hover(|s| s.bg(theme.accent_rgba().opacity(0.25)))
        .active(|s| s.bg(theme.accent_rgba().opacity(0.4)))
        .on_drag(edge, |_, _, _, cx| cx.new(|_| ResizeDragGhost))
        .on_mouse_down(MouseButton::Left, {
            let edge = edge;
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.begin_panel_resize(edge, event.position.x, cx);
            })
        })
        .on_drag_move::<PanelResizeEdge>(cx.listener(
            |this, event: &DragMoveEvent<PanelResizeEdge>, _, cx| {
                this.handle_panel_resize_drag(event, cx);
            },
        ))
}
