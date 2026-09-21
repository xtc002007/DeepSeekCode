//! Session viewport: activity timeline rendering and scroll helpers.

use deepseekcode_schema::activity::ActivityTree;
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

/// Render the Cursor-style activity timeline (delegates to `ActivityTimelineView`).
pub fn render_activity_timeline(
    cx: &Context<crate::workspace::WorkspaceView>,
    tree: &ActivityTree,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    scroll_handle: &ScrollHandle,
    session_loading: bool,
) -> impl IntoElement {
    crate::activity_timeline::ActivityTimelineView::render(
        cx,
        tree,
        expanded_groups,
        collapsed_groups,
        expanded_leaves,
        highlighted_todo_leaf,
        scroll_handle,
        session_loading,
    )
}
