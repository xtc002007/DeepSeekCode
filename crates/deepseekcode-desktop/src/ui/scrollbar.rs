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
                .right(px(10.))
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

    // P1-8: don't render a fake slot when there's nothing to scroll —
    // previously an empty list still showed a full-height gray bar + 6px pad.
    if !scrollable {
        return div().id(track_id).w(px(0.)).into_any_element();
    }

    let content_height = viewport + max_scroll;
    let mut thumb_h = viewport * (viewport / content_height);
    if thumb_h < SCROLLBAR_MIN_THUMB {
        thumb_h = SCROLLBAR_MIN_THUMB;
    }
    if thumb_h > viewport {
        thumb_h = viewport;
    }
    // P1-8: GPUI scroll offsets are negative when scrolled down; guard both
    // signs so a future sign flip can't invert the thumb.
    let progress = (-offset.y / max_scroll).clamp(0.0, 1.0);
    let travel = viewport - thumb_h;
    let thumb_top = travel * progress;

    let handle = scroll_handle.clone();
    let handle_for_move = scroll_handle.clone();
    let handle_for_up = scroll_handle.clone();
    let handle_for_thumb = scroll_handle.clone();
    // P1-8: wider hit area (10px) with a centered 4px visual so touch/high-DPI
    // can actually grab it; visual stays slim.
    let track = div()
        .id(track_id)
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(10.))
        .bg(gpui::hsla(0., 0., 0., 0.))
        .hover(|s| s.bg(theme.bg_hover))
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                // P1-8: track geometry == viewport geometry (track is
                // top_0/bottom_0 of the same relative parent), so the
                // viewport origin/height is the correct denominator.
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
        )
        // P1-8: true thumb drag — move updates the offset proportionally.
        .on_mouse_move(
            cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let Some((start_y, start_off)) = this.scrollbar_drag else {
                    return;
                };
                let bounds = handle_for_move.bounds();
                let track_height = bounds.size.height;
                if track_height <= px(0.) {
                    return;
                }
                let max_scroll = handle_for_move.max_offset().y;
                if max_scroll <= px(0.) {
                    return;
                }
                let dy = event.position.y - px(start_y);
                let viewport = bounds.size.height;
                let content_height = viewport + max_scroll;
                // thumb travel <-> content scroll mapping.
                let mut thumb_h = viewport * (viewport / content_height);
                if thumb_h < SCROLLBAR_MIN_THUMB {
                    thumb_h = SCROLLBAR_MIN_THUMB;
                }
                let travel = (viewport - thumb_h).max(px(1.));
                let ratio_delta = dy / travel;
                let new_off = (px(start_off) - max_scroll * ratio_delta).clamp(-max_scroll, px(0.));
                handle_for_move.set_offset(point(px(0.), new_off));
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.scrollbar_drag = None;
                let _ = &handle_for_up;
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(
            div()
                .absolute()
                .top(thumb_top)
                .left(px(3.))
                .right(px(3.))
                .h(thumb_h)
                .rounded_full()
                .bg(theme.text_muted)
                .hover(|s| s.bg(theme.text_primary))
                // P1-8: thumb press starts a drag (and swallows the track
                // jump that would otherwise fire from under the cursor).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let cur_off: f32 = handle_for_thumb.offset().y.into();
                        this.scrollbar_drag = Some((f32::from(event.position.y), cur_off));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        );
    track.into_any_element()
}
