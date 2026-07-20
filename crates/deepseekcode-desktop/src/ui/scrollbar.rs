//! Vertical scrollbar matching the session timeline pattern.

use gpui::*;

pub const SCROLLBAR_WIDTH: Pixels = px(6.);
const SCROLLBAR_MIN_THUMB: Pixels = px(24.);

/// Flex child scroll viewport (`flex_1` + `relative` + absolute scroll area).
/// Parent chain must be `.flex().flex_col()` with `min_h(px(0.))` or height collapses to zero.
pub fn render_flex_scroll_viewport(
    cx: &Context<crate::workspace::WorkspaceView>,
    scroll_handle: &ScrollHandle,
    scroll_id: impl Into<ElementId>,
    track_id: &'static str,
    mark_session_follow: bool,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .w_full()
        .relative()
        .child(
            div()
                .id(scroll_id)
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(SCROLLBAR_WIDTH)
                .track_scroll(scroll_handle)
                .overflow_y_scroll()
                .child(content),
        )
        .child(render_vertical_scrollbar(
            cx,
            scroll_handle,
            track_id,
            mark_session_follow,
        ))
}

pub fn render_vertical_scrollbar(
    cx: &Context<crate::workspace::WorkspaceView>,
    scroll_handle: &ScrollHandle,
    track_id: &'static str,
    mark_session_follow: bool,
) -> impl IntoElement {
    let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
    let offset = scroll_handle.offset();
    let max_offset = scroll_handle.max_offset();
    let viewport = scroll_handle.bounds().size.height;
    let max_scroll = max_offset.y;
    let scrollable = max_scroll > px(0.) && viewport > px(0.);

    let (thumb_height, thumb_top) = if scrollable {
        let content_height = viewport + max_scroll;
        let mut thumb_h = viewport * (viewport / content_height);
        if thumb_h < SCROLLBAR_MIN_THUMB {
            thumb_h = SCROLLBAR_MIN_THUMB;
        }
        if thumb_h > viewport {
            thumb_h = viewport;
        }
        let progress = (-offset.y / max_scroll).clamp(0.0, 1.0);
        let travel = viewport - thumb_h;
        (thumb_h, travel * progress)
    } else {
        (viewport.max(SCROLLBAR_MIN_THUMB), px(0.))
    };

    let handle = scroll_handle.clone();
    let mut track = div()
        .id(track_id)
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(SCROLLBAR_WIDTH)
        .bg(theme.bg_surface)
        .border_l_1()
        .border_color(theme.border_default)
        .cursor(CursorStyle::PointingHand);

    if scrollable {
        track = track.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                let bounds = handle.bounds();
                let track_top = bounds.origin.y;
                let track_height = bounds.size.height;
                if track_height <= px(0.) {
                    return;
                }
                let click_y = event.position.y - track_top;
                let ratio = (click_y / track_height).clamp(0.0, 1.0);
                let max_scroll = handle.max_offset().y;
                handle.set_offset(point(px(0.), -max_scroll * ratio));
                if mark_session_follow {
                    this.mark_session_scroll_position(&handle);
                }
                cx.stop_propagation();
                cx.notify();
            }),
        );
    }

    track.child(
        div()
            .absolute()
            .top(thumb_top)
            .left(px(2.))
            .right(px(2.))
            .h(thumb_height)
            .rounded_sm()
            .bg(if scrollable {
                theme.text_muted
            } else {
                theme.border_default
            }),
    )
}
