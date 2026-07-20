//! Automated coverage for ui-update-plan.md section 6.2 (desktop UI items).

#[cfg(test)]
mod tests {
    use crate::activity_leaf::format_leaf_line;
    use crate::logic::{
        activity_trees_equivalent, classify_git_porcelain, event_affects_activity_tree,
        find_todo_leaf_id_for_index, plan_activity_click_route, patch_running_tool_leaf_preview,
        ActivityClickRoute,
    };
    use deepseekcode_sdk::ActivityProjector;
    use deepseekcode_schema::activity::{
        ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode, ActivityStatus,
        ActivityTargetKind, ActivityTree,
    };
    use deepseekcode_schema::event::EventKind;

    #[test]
    fn acceptance_read_leaf_single_line_no_json() {
        let leaf = ActivityLeaf {
            id: "r1".into(),
            kind: ActivityKind::Read,
            verb: "Read".into(),
            target: ActivityTargetKind::File {
                path: "app/page.js".into(),
                line_start: Some(1),
                line_end: Some(17),
            },
            meta: ActivityMeta::default(),
            status: ActivityStatus::Completed,
            detail_ref: Some("r1".into()),
            text: None,
        };
        let line = format_leaf_line(&leaf);
        assert!(line.contains("app/page.js"));
        assert!(line.contains("L1-17"));
        assert!(!line.contains('{'));
    }

    #[test]
    fn acceptance_click_read_opens_editor_at_line() {
        let route = plan_activity_click_route(
            ActivityKind::Read,
            &ActivityTargetKind::File {
                path: "src/main.rs".into(),
                line_start: Some(42),
                line_end: Some(50),
            },
            &ActivityMeta::default(),
            Some("r1"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Editor {
                path: "src/main.rs".into(),
                line: Some(42),
            }
        );
    }

    #[test]
    fn acceptance_edit_leaf_shows_diff_stats() {
        let leaf = ActivityLeaf {
            id: "e1".into(),
            kind: ActivityKind::Edit,
            verb: "Edited".into(),
            target: ActivityTargetKind::File {
                path: "foo.rs".into(),
                line_start: None,
                line_end: None,
            },
            meta: ActivityMeta {
                diff_stats: Some(deepseekcode_schema::activity::DiffStats {
                    added: 21,
                    removed: 1,
                }),
                ..Default::default()
            },
            status: ActivityStatus::Completed,
            detail_ref: Some("e1".into()),
            text: None,
        };
        let line = format_leaf_line(&leaf);
        assert!(line.contains("+21"));
        assert!(line.contains("-1"));
    }

    #[test]
    fn acceptance_bash_click_routes_to_terminal() {
        let route = plan_activity_click_route(
            ActivityKind::Bash,
            &ActivityTargetKind::Command {
                command: "npm test".into(),
            },
            &ActivityMeta {
                exit_code: Some(0),
                ..Default::default()
            },
            Some("bash-1"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Terminal {
                detail_ref: "bash-1".into()
            }
        );
    }

    #[test]
    fn acceptance_todo_bidirectional_highlight_mapping() {
        let tree = ActivityTree {
            roots: vec![ActivityNode::Leaf(ActivityLeaf {
                id: "todo-9".into(),
                kind: ActivityKind::TodoWrite,
                verb: "Updated todos".into(),
                target: ActivityTargetKind::TodoList,
                meta: ActivityMeta {
                    todo_focus_index: Some(2),
                    ..Default::default()
                },
                status: ActivityStatus::Completed,
                detail_ref: Some("todo-9".into()),
                text: None,
            })],
        };
        assert_eq!(find_todo_leaf_id_for_index(&tree, 2).as_deref(), Some("todo-9"));
    }

    #[test]
    fn acceptance_git_panel_groups_staged_unstaged() {
        let lines = vec![
            "M  staged.rs".into(),
            " M unstaged.rs".into(),
            "?? new.rs".into(),
        ];
        let (staged, unstaged, untracked) = classify_git_porcelain(&lines);
        assert_eq!(staged.len(), 1);
        assert_eq!(unstaged.len(), 1);
        assert_eq!(untracked.len(), 1);
    }

    #[test]
    fn acceptance_tool_progress_skips_full_reproject() {
        assert!(!event_affects_activity_tree(EventKind::ToolProgress));
    }

    #[test]
    fn acceptance_running_bash_patch_without_reproject() {
        let mut tree = ActivityTree {
            roots: vec![ActivityNode::Leaf(ActivityLeaf {
                id: "bash-1".into(),
                kind: ActivityKind::Bash,
                verb: "Ran".into(),
                target: ActivityTargetKind::Command {
                    command: "npm install".into(),
                },
                meta: ActivityMeta::default(),
                status: ActivityStatus::Running,
                detail_ref: Some("bash-1".into()),
                text: None,
            })],
        };
        assert!(patch_running_tool_leaf_preview(&mut tree, "bash-1", "[stdout] working"));
    }

    #[test]
    fn acceptance_hydrate_equivalence_helper() {
        let events = vec![];
        let a = ActivityProjector::project(&events, &[]);
        let b = ActivityProjector::project(&events, &[]);
        assert!(activity_trees_equivalent(&a, &b));
    }

    #[test]
    fn acceptance_edit_click_routes_to_editor_with_diff_stats() {
        use deepseekcode_schema::activity::DiffStats;
        let route = plan_activity_click_route(
            ActivityKind::Edit,
            &ActivityTargetKind::File {
                path: "foo.rs".into(),
                line_start: None,
                line_end: None,
            },
            &ActivityMeta {
                diff_stats: Some(DiffStats { added: 21, removed: 1 }),
                ..Default::default()
            },
            Some("e1"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Editor {
                path: "foo.rs".into(),
                line: None,
            }
        );
    }

    #[test]
    fn acceptance_mcp_routes_to_editor_when_path_in_metadata() {
        let route = plan_activity_click_route(
            ActivityKind::Mcp,
            &ActivityTargetKind::Generic {
                label: "mcp_read".into(),
            },
            &ActivityMeta {
                first_match_path: Some("src/lib.rs".into()),
                first_match_line: Some(12),
                ..Default::default()
            },
            Some("m1"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Editor {
                path: "src/lib.rs".into(),
                line: Some(12),
            }
        );
    }

    #[test]
    fn acceptance_question_route_shows_detail() {
        let route = plan_activity_click_route(
            ActivityKind::Question,
            &ActivityTargetKind::Generic {
                label: "Pick one".into(),
            },
            &ActivityMeta::default(),
            Some("q1"),
        );
        assert_eq!(route, ActivityClickRoute::ShowDetail);
    }

    #[test]
    fn acceptance_provider_turn_group_expanded_by_default() {
        use crate::logic::is_activity_group_expanded;
        use deepseekcode_schema::activity::{ActivityGroup, ActivityGroupKind, ActivitySummary};
        use std::collections::HashSet;
        let group = ActivityGroup {
            id: "turn-1".into(),
            kind: ActivityGroupKind::ProviderTurn,
            label: "Worked for 2s".into(),
            summary: Some(ActivitySummary::default()),
            children: vec![],
            collapsed_default: false,
        };
        let expanded = HashSet::new();
        let collapsed = HashSet::new();
        assert!(is_activity_group_expanded(&group, &expanded, &collapsed));
    }

    #[test]
    fn acceptance_todo_scroll_row_index_found() {
        use crate::logic::activity_visible_row_index;
        use std::collections::HashSet;
        let tree = ActivityTree {
            roots: vec![ActivityNode::Group(deepseekcode_schema::activity::ActivityGroup {
                id: "turn-1".into(),
                kind: deepseekcode_schema::activity::ActivityGroupKind::ProviderTurn,
                label: "Worked".into(),
                summary: None,
                children: vec![ActivityNode::Leaf(ActivityLeaf {
                    id: "todo-9".into(),
                    kind: ActivityKind::TodoWrite,
                    verb: "Updated todos".into(),
                    target: ActivityTargetKind::TodoList,
                    meta: ActivityMeta::default(),
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: None,
                })],
                collapsed_default: false,
            })],
        };
        let expanded = HashSet::new();
        let collapsed = HashSet::new();
        let leaves = HashSet::new();
        assert_eq!(
            activity_visible_row_index(&tree, "todo-9", &expanded, &collapsed, &leaves),
            Some(1)
        );
    }
}
