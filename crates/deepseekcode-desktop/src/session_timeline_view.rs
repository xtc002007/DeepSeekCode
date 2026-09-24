//! Session timeline view boundary — virtualized activity rendering state.

use deepseekcode_schema::activity::ActivityTree;
use gpui::ScrollHandle;
use std::collections::HashSet;

use crate::activity_rows::flatten_activity_tree;

/// Rendering snapshot for the session activity timeline (decoupled from WorkspaceView shell).
#[derive(Debug, Clone)]
pub struct SessionTimelineView {
    pub tree: ActivityTree,
    pub expanded_groups: HashSet<String>,
    pub collapsed_groups: HashSet<String>,
    pub expanded_leaves: HashSet<String>,
    pub generation: u64,
}

impl SessionTimelineView {
    pub fn from_store(
        tree: &ActivityTree,
        expanded_groups: &HashSet<String>,
        collapsed_groups: &HashSet<String>,
        expanded_leaves: &HashSet<String>,
        generation: u64,
    ) -> Self {
        Self {
            tree: tree.clone(),
            expanded_groups: expanded_groups.clone(),
            collapsed_groups: collapsed_groups.clone(),
            expanded_leaves: expanded_leaves.clone(),
            generation,
        }
    }

    pub fn flattened_rows(&self) -> Vec<crate::activity_rows::FlattenedActivityRow> {
        flatten_activity_tree(
            &self.tree,
            &self.expanded_groups,
            &self.collapsed_groups,
            &self.expanded_leaves,
        )
    }

    pub fn render(
        &self,
        cx: &gpui::Context<crate::workspace::WorkspaceView>,
        highlighted_todo_leaf: Option<&str>,
        selected_detail_key: Option<&str>,
        scroll_handle: &ScrollHandle,
        session_loading: bool,
        task_active: bool,
        text_focus: &gpui::FocusHandle,
    ) -> impl gpui::IntoElement {
        crate::activity_timeline::ActivityTimelineView::render(
            cx,
            &self.tree,
            &self.expanded_groups,
            &self.collapsed_groups,
            &self.expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            scroll_handle,
            session_loading,
            task_active,
            text_focus,
        )
    }
}
