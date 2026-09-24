//! Session viewport: scroll helpers for the activity timeline.

use gpui::*;

pub fn is_session_at_bottom(scroll_handle: &ScrollHandle) -> bool {
    let max_scroll = scroll_handle.max_offset().y;
    // max_scroll == 0 often means layout has not measured yet (or content remounted).
    // Do NOT treat that as "at bottom" — it fights wheel scrolling and snaps to tail.
    if max_scroll <= px(0.) {
        return false;
    }
    -scroll_handle.offset().y >= max_scroll - px(24.)
}
