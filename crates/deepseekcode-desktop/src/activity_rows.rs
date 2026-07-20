//! Flattened visible activity rows for timeline virtual scrolling.

use deepseekcode_schema::activity::{
    ActivityGroup, ActivityGroupKind, ActivityKind, ActivityLeaf, ActivityNode, ActivitySummary,
    ActivityTree,
};
use std::collections::HashSet;

use crate::logic::is_activity_group_expanded;

pub const COMPACT_ROW_HEIGHT_PX: f32 = 28.0;
pub const USER_MESSAGE_ROW_HEIGHT_PX: f32 = 120.0;
pub const ASSISTANT_HEADER_HEIGHT_PX: f32 = 36.0;
pub const MARKDOWN_LINE_HEIGHT_PX: f32 = 18.0;
pub const GROUP_HEADER_HEIGHT_PX: f32 = 32.0;
pub const GROUP_SUMMARY_HEIGHT_PX: f32 = 24.0;
pub const DEFAULT_VIEWPORT_HEIGHT_PX: f32 = 720.0;
pub const OVERSCAN_ROWS: usize = 6;

#[derive(Debug, Clone)]
pub enum FlattenedActivityRow {
    GroupHeader {
        group_id: String,
        label: String,
        depth: usize,
        collapsed_default: bool,
        kind: ActivityGroupKind,
    },
    GroupSummary {
        summary: ActivitySummary,
        depth: usize,
    },
    Leaf {
        leaf: ActivityLeaf,
        depth: usize,
        message_expanded: bool,
    },
}

impl FlattenedActivityRow {
    pub fn estimated_height_px(&self) -> f32 {
        match self {
            FlattenedActivityRow::GroupHeader { .. } => GROUP_HEADER_HEIGHT_PX,
            FlattenedActivityRow::GroupSummary { .. } => GROUP_SUMMARY_HEIGHT_PX,
            FlattenedActivityRow::Leaf {
                leaf,
                message_expanded,
                ..
            } => estimate_leaf_height(leaf, *message_expanded),
        }
    }
}

pub fn estimate_leaf_height(leaf: &ActivityLeaf, message_expanded: bool) -> f32 {
    use deepseekcode_schema::activity::ActivityKind;
    match leaf.kind {
        ActivityKind::UserMessage => {
            if leaf.text.is_some() {
                USER_MESSAGE_ROW_HEIGHT_PX
            } else {
                COMPACT_ROW_HEIGHT_PX
            }
        }
        ActivityKind::AssistantMessage => {
            if let Some(text) = &leaf.text {
                let lines = text.lines().count().max(1) as f32;
                ASSISTANT_HEADER_HEIGHT_PX + (lines * MARKDOWN_LINE_HEIGHT_PX).min(480.0)
            } else {
                COMPACT_ROW_HEIGHT_PX
            }
        }
        ActivityKind::Reasoning if message_expanded && leaf.text.is_some() => {
            let lines = leaf.text.as_ref().map(|t| t.lines().count()).unwrap_or(1) as f32;
            COMPACT_ROW_HEIGHT_PX + (lines * MARKDOWN_LINE_HEIGHT_PX).min(240.0)
        }
        ActivityKind::SessionMeta => COMPACT_ROW_HEIGHT_PX,
        _ => COMPACT_ROW_HEIGHT_PX,
    }
}

pub fn flatten_activity_tree(
    tree: &ActivityTree,
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
    expanded_leaves: &HashSet<String>,
) -> Vec<FlattenedActivityRow> {
    let mut rows = Vec::new();
    flatten_nodes(
        &tree.roots,
        expanded_groups,
        collapsed_groups,
        expanded_leaves,
        0,
        &mut rows,
    );
    rows
}

fn flatten_nodes(
    nodes: &[ActivityNode],
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
    expanded_leaves: &HashSet<String>,
    depth: usize,
    rows: &mut Vec<FlattenedActivityRow>,
) {
    use deepseekcode_schema::activity::{ActivityGroupKind, ActivityNode};
    for node in nodes {
        match node {
            ActivityNode::Group(group) => {
                flatten_group(
                    group,
                    expanded_groups,
                    collapsed_groups,
                    expanded_leaves,
                    depth,
                    rows,
                );
            }
            ActivityNode::Leaf(leaf) => {
                let message_expanded = expanded_leaves.contains(&leaf.id)
                    && leaf.text.is_some()
                    && matches!(
                        leaf.kind,
                        ActivityKind::Reasoning | ActivityKind::AssistantMessage
                    );
                rows.push(FlattenedActivityRow::Leaf {
                    leaf: leaf.clone(),
                    depth,
                    message_expanded,
                });
            }
        }
    }
}

fn flatten_group(
    group: &ActivityGroup,
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
    expanded_leaves: &HashSet<String>,
    depth: usize,
    rows: &mut Vec<FlattenedActivityRow>,
) {
    use deepseekcode_schema::activity::ActivityGroupKind;
    let expanded = is_activity_group_expanded(group, expanded_groups, collapsed_groups);
    rows.push(FlattenedActivityRow::GroupHeader {
        group_id: group.id.clone(),
        label: group.label.clone(),
        depth,
        collapsed_default: group.collapsed_default,
        kind: group.kind,
    });
    if expanded {
        if let Some(summary) = &group.summary {
            rows.push(FlattenedActivityRow::GroupSummary {
                summary: summary.clone(),
                depth,
            });
        }
        flatten_nodes(
            &group.children,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            depth + 1,
            rows,
        );
    }
}

pub fn visible_row_range(
    rows: &[FlattenedActivityRow],
    scroll_y_px: f32,
    viewport_height_px: f32,
) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let mut y = 0.0f32;
    let mut start = rows.len();
    let mut end = 0usize;
    let view_top = scroll_y_px.max(0.0);
    let view_bottom = view_top + viewport_height_px.max(1.0);
    let overscan_px = OVERSCAN_ROWS as f32 * COMPACT_ROW_HEIGHT_PX;

    for (index, row) in rows.iter().enumerate() {
        let height = row.estimated_height_px();
        let row_top = y;
        let row_bottom = y + height;
        y = row_bottom;
        if row_bottom >= view_top - overscan_px && row_top <= view_bottom + overscan_px {
            start = start.min(index);
            end = index + 1;
        }
    }
    if end <= start {
        (0, rows.len().min(32))
    } else {
        (start, end.min(rows.len()))
    }
}

pub fn prefix_height_px(rows: &[FlattenedActivityRow], end_index: usize) -> f32 {
    rows.iter()
        .take(end_index)
        .map(FlattenedActivityRow::estimated_height_px)
        .sum()
}

pub fn suffix_height_px(rows: &[FlattenedActivityRow], start_index: usize) -> f32 {
    rows.iter()
        .skip(start_index)
        .map(FlattenedActivityRow::estimated_height_px)
        .sum()
}

pub fn total_height_px(rows: &[FlattenedActivityRow]) -> f32 {
    rows.iter()
        .map(FlattenedActivityRow::estimated_height_px)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_schema::activity::{
        ActivityGroup, ActivityGroupKind, ActivityKind, ActivityLeaf, ActivityMeta,
        ActivityNode, ActivityStatus, ActivityTargetKind, ActivityTree,
    };

    fn sample_tree() -> ActivityTree {
        ActivityTree {
            roots: (0..40)
                .map(|i| {
                    ActivityNode::Leaf(ActivityLeaf {
                        id: format!("leaf-{i}"),
                        kind: ActivityKind::Bash,
                        verb: "Run".into(),
                        target: ActivityTargetKind::None,
                        meta: ActivityMeta::default(),
                        status: ActivityStatus::Completed,
                        detail_ref: None,
                        text: Some(format!("command {i}")),
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn flatten_produces_one_row_per_leaf() {
        let tree = sample_tree();
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        assert_eq!(rows.len(), 40);
    }

    #[test]
    fn visible_range_culls_distant_rows() {
        let tree = sample_tree();
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        let (start, end) = visible_row_range(&rows, 400.0, 200.0);
        assert!(end - start < 40);
        assert!(start > 0 || end < 40);
    }
}
