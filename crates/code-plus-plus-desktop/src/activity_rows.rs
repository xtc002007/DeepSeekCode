//! Flattened visible activity rows for timeline virtual scrolling.

use code_plus_plus_schema::activity::{
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
        /// Structured whole-second span for turn headers. This is what the
        /// UI renders; `label` stays as the backend's English fallback.
        duration_secs: Option<u64>,
        /// Turn summary, carried for header composition
        /// (`Explore 5 files, Worked for 12s`). Turns never emit a separate
        /// `GroupSummary` row; only non-turn groups use that variant.
        summary: Option<ActivitySummary>,
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
    pub fn depth(&self) -> usize {
        match self {
            FlattenedActivityRow::GroupHeader { depth, .. } => *depth,
            FlattenedActivityRow::GroupSummary { depth, .. } => *depth,
            FlattenedActivityRow::Leaf { depth, .. } => *depth,
        }
    }

    /// Same row with `depth` shifted (saturating). Used to render a turn's
    /// body rows inside the indented rail container without double indent.
    pub fn shifted(&self, sub: usize) -> Self {
        let mut row = self.clone();
        let depth = row.depth().saturating_sub(sub);
        match &mut row {
            FlattenedActivityRow::GroupHeader { depth: d, .. } => *d = depth,
            FlattenedActivityRow::GroupSummary { depth: d, .. } => *d = depth,
            FlattenedActivityRow::Leaf { depth: d, .. } => *d = depth,
        }
        row
    }

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
    use code_plus_plus_schema::activity::ActivityKind;
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

/// Presentational orphan containment (display hierarchy fix).
///
/// Tool leaves that landed on `tree.roots` without an active turn (backend
/// fallback `else roots.push`) would otherwise render as dangling rows with
/// no card above them. Here they are adopted by the nearest `ProviderTurn`:
/// orphans preceding a turn are prepended to its children (their results feed
/// that turn), trailing orphans are appended to the previous turn.
/// `UserMessage` leaves always stay top-level (conversation boundaries).
/// Purely presentational: the backend tree is never mutated.
pub(crate) fn attach_orphan_leaves(roots: &[ActivityNode]) -> Vec<ActivityNode> {
    use code_plus_plus_schema::activity::ActivityKind;
    fn is_turn(node: &ActivityNode) -> bool {
        matches!(node, ActivityNode::Group(g) if g.kind == ActivityGroupKind::ProviderTurn)
    }
    fn is_adoptable(node: &ActivityNode) -> bool {
        matches!(node, ActivityNode::Leaf(leaf) if leaf.kind != ActivityKind::UserMessage)
    }

    let mut out: Vec<ActivityNode> = Vec::with_capacity(roots.len());
    let mut pending: Vec<ActivityNode> = Vec::new();
    let mut last_turn_idx: Option<usize> = None;
    for node in roots {
        if is_turn(node) {
            let mut group = match node.clone() {
                ActivityNode::Group(g) => g,
                ActivityNode::Leaf(_) => unreachable!(),
            };
            if !pending.is_empty() {
                let mut children = std::mem::take(&mut pending);
                children.extend(group.children.drain(..));
                group.children = children;
            }
            out.push(ActivityNode::Group(group));
            last_turn_idx = Some(out.len() - 1);
        } else if is_adoptable(node) {
            pending.push(node.clone());
        } else {
            // UserMessage leaves and non-turn groups stay top-level and do
            // not break orphan adoption (pending still flows to next turn).
            out.push(node.clone());
        }
    }
    if !pending.is_empty() {
        if let Some(idx) = last_turn_idx {
            if let ActivityNode::Group(group) = &mut out[idx] {
                group.children.extend(pending.drain(..));
            }
        }
        // No turn at all: leave trailing orphans top-level.
        out.extend(pending.drain(..));
    }
    lead_with_assistant_message(&mut out);
    out
}

/// Narrative order ("tell, then do"): the assistant's thinking/planning text
/// states intent before the tool operations that follow it, so it leads the
/// turn's children. Stable partition — relative order inside each class keeps
/// document order. Only root-level turns are reordered; nested sub-turns
/// keep backend order.
fn lead_with_assistant_message(nodes: &mut Vec<ActivityNode>) {
    use code_plus_plus_schema::activity::ActivityKind;
    for node in nodes.iter_mut() {
        let ActivityNode::Group(group) = node else {
            continue;
        };
        if group.kind != ActivityGroupKind::ProviderTurn {
            continue;
        }
        let mut assistants = Vec::new();
        let mut tools = Vec::with_capacity(group.children.len());
        for child in group.children.drain(..) {
            match &child {
                ActivityNode::Leaf(leaf) if leaf.kind == ActivityKind::AssistantMessage => {
                    assistants.push(child)
                }
                _ => tools.push(child),
            }
        }
        assistants.extend(tools);
        group.children = assistants;
    }
}

/// Single display-preparation entry: mark stale Running leaves (10min
/// watchdog, wall-clock now) then adopt orphans. All render AND index paths
/// must go through here so displayed rows and computed indices never drift.
/// Backend tree is cloned, never mutated.
pub fn adopt_orphans_for_display(tree: &ActivityTree) -> Vec<ActivityNode> {
    let mut owned = tree.roots.clone();
    mark_stale_for_display(&mut owned);
    attach_orphan_leaves(&owned)
}

fn mark_stale_for_display(nodes: &mut [ActivityNode]) {
    use code_plus_plus_schema::activity::ActivityStatus;
    let now = chrono::Utc::now();
    let timeout = code_plus_plus_core::activity::projector::STALE_RUNNING_TIMEOUT_SECS;
    for node in nodes.iter_mut() {
        match node {
            ActivityNode::Group(g) => mark_stale_for_display(&mut g.children),
            ActivityNode::Leaf(l) => {
                if l.status != ActivityStatus::Running {
                    continue;
                }
                if let Some(started) = l.started_at {
                    if (now - started).num_seconds() >= timeout
                        && (now - started).num_seconds() >= 0
                    {
                        l.status = ActivityStatus::Stale;
                    }
                }
            }
        }
    }
}

pub fn flatten_activity_tree(
    tree: &ActivityTree,
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
    expanded_leaves: &HashSet<String>,
) -> Vec<FlattenedActivityRow> {
    let mut rows = Vec::new();
    let owned = adopt_orphans_for_display(tree);
    flatten_nodes(
        &owned,
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
    use code_plus_plus_schema::activity::ActivityNode;
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
    use code_plus_plus_schema::activity::ActivityGroupKind;
    let expanded = is_activity_group_expanded(group, expanded_groups, collapsed_groups);
    // Turn summaries live in the header (`Explore 5 files, Worked for 12s`),
    // so turns never emit a separate summary row. Non-turn groups keep the
    // legacy path (they never carry summaries today, kept for safety).
    let is_turn = group.kind == ActivityGroupKind::ProviderTurn;
    rows.push(FlattenedActivityRow::GroupHeader {
        group_id: group.id.clone(),
        label: group.label.clone(),
        depth,
        collapsed_default: group.collapsed_default,
        kind: group.kind,
        duration_secs: group.duration_secs,
        summary: group.summary.clone(),
    });
    if expanded {
        if let Some(summary) = &group.summary {
            if !is_turn {
                rows.push(FlattenedActivityRow::GroupSummary {
                    summary: summary.clone(),
                    depth,
                });
            }
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
    use code_plus_plus_schema::activity::{
        ActivityGroup, ActivityGroupKind, ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode,
        ActivityStatus, ActivityTargetKind, ActivityTree,
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
                        started_at: None,
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

    fn tool_leaf(id: &str, kind: ActivityKind) -> ActivityNode {
        ActivityNode::Leaf(ActivityLeaf {
            id: id.into(),
            kind,
            verb: "Listed".into(),
            target: ActivityTargetKind::None,
            meta: ActivityMeta::default(),
            status: ActivityStatus::Completed,
            detail_ref: None,
            text: None,
            started_at: None,
        })
    }

    fn turn_group(id: &str, children: Vec<ActivityNode>) -> ActivityNode {
        ActivityNode::Group(ActivityGroup {
            id: id.into(),
            kind: ActivityGroupKind::ProviderTurn,
            label: "Worked for 1s".into(),
            duration_secs: Some(1),
            summary: None,
            children,
            collapsed_default: false,
        })
    }

    fn turn_group_with_summary(
        id: &str,
        summary: ActivitySummary,
        children: Vec<ActivityNode>,
    ) -> ActivityNode {
        ActivityNode::Group(ActivityGroup {
            id: id.into(),
            kind: ActivityGroupKind::ProviderTurn,
            label: "Worked for 12s".into(),
            duration_secs: Some(12),
            summary: Some(summary),
            children,
            collapsed_default: false,
        })
    }

    #[test]
    fn orphan_leaves_are_adopted_by_following_turn() {
        let tree = ActivityTree {
            roots: vec![
                tool_leaf("orphan", ActivityKind::List),
                turn_group("turn-1", vec![tool_leaf("inner", ActivityKind::Read)]),
            ],
        };
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        // header + adopted orphan + inner child (no summary on this group).
        assert_eq!(rows.len(), 3);
        assert!(matches!(
            &rows[0],
            FlattenedActivityRow::GroupHeader { summary: None, .. }
        ));
        let ids: Vec<&str> = rows[1..]
            .iter()
            .filter_map(|r| match r {
                FlattenedActivityRow::Leaf { leaf, .. } => Some(leaf.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["orphan", "inner"]);
    }

    #[test]
    fn trailing_orphans_join_previous_turn_and_user_messages_stay() {
        let tree = ActivityTree {
            roots: vec![
                turn_group("turn-1", vec![]),
                tool_leaf("trailing", ActivityKind::Bash),
                ActivityNode::Leaf(ActivityLeaf {
                    id: "user-1".into(),
                    kind: ActivityKind::UserMessage,
                    verb: "You".into(),
                    target: ActivityTargetKind::None,
                    meta: ActivityMeta::default(),
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: Some("hi".into()),
                    started_at: None,
                }),
            ],
        };
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        // header + adopted trailing + user message stays top-level.
        assert_eq!(rows.len(), 3);
        let ids: Vec<&str> = rows[1..]
            .iter()
            .filter_map(|r| match r {
                FlattenedActivityRow::Leaf { leaf, .. } => Some(leaf.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["trailing", "user-1"]);
    }

    #[test]
    fn turn_summary_rides_on_header_without_summary_row() {
        let tree = ActivityTree {
            roots: vec![turn_group_with_summary(
                "turn-1",
                ActivitySummary {
                    files_explored: 5,
                    ..Default::default()
                },
                vec![tool_leaf("a", ActivityKind::List)],
            )],
        };
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        // header + child only: no separate GroupSummary row for turns.
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            &rows[0],
            FlattenedActivityRow::GroupHeader {
                summary: Some(_),
                ..
            }
        ));
        assert!(!rows
            .iter()
            .any(|r| matches!(r, FlattenedActivityRow::GroupSummary { .. })));
    }

    #[test]
    fn assistant_leaf_leads_tool_rows() {
        // Narrative order: intent first, operations after.
        let tree = ActivityTree {
            roots: vec![turn_group(
                "turn-1",
                vec![
                    ActivityNode::Leaf(ActivityLeaf {
                        id: "assistant-1".into(),
                        kind: ActivityKind::AssistantMessage,
                        verb: "Assistant".into(),
                        target: ActivityTargetKind::None,
                        meta: ActivityMeta::default(),
                        status: ActivityStatus::Completed,
                        detail_ref: None,
                        text: Some("done".into()),
                        started_at: None,
                    }),
                    tool_leaf("a", ActivityKind::List),
                ],
            )],
        };
        let rows = flatten_activity_tree(&tree, &HashSet::new(), &HashSet::new(), &HashSet::new());
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                FlattenedActivityRow::Leaf { leaf, .. } => Some(leaf.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["assistant-1", "a"]);
    }
}
