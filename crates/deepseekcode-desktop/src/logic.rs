//! Pure logic helpers for desktop (unit-testable without GPUI).

use deepseekcode_schema::activity::{
    ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode, ActivityStatus, ActivityTargetKind,
    ActivityTree,
};
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::message::{Message, MessageRole, PartKind};
use deepseekcode_schema::session::Session;
use deepseekcode_schema::todo::TodoItem;
use deepseekcode_types::SessionId;
use serde_json::Value;
use std::collections::HashSet;

pub fn agent_selector_for_kind(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Plan => "plan",
        AgentKind::General => "general",
        AgentKind::Build => "build",
    }
}

pub fn implement_prompt_from_plan_path(plan_path: &str) -> String {
    format!(
        "请严格按照以下方案分步实现：{plan_path}\n先从方案中的第一步开始；若方案有阶段划分，完成一阶段后暂停等我确认。"
    )
}

pub fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        0
    } else {
        (text.len() as u32 / 4).max(1)
    }
}

/// Read token usage from a `message.completed` event payload, if present.
pub fn token_usage_from_payload(payload: &Value) -> Option<(u32, u32, u32)> {
    let input = payload.get("input_tokens")?.as_u64()? as u32;
    let output = payload.get("output_tokens")?.as_u64()? as u32;
    let reasoning = payload
        .get("reasoning_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Some((input, output, reasoning))
}

/// Latest context fill from session events (`input_tokens` on the most recent completed message).
pub fn latest_context_usage_from_events(events: &[deepseekcode_schema::event::EventEnvelope]) -> Option<(u32, u32, u32)> {
    use deepseekcode_schema::event::EventKind;
    events
        .iter()
        .rev()
        .find_map(|event| {
            if event.kind != EventKind::MessageCompleted {
                return None;
            }
            token_usage_from_payload(&event.payload)
        })
}

pub fn parse_unified_diff_rows(hunks: &str) -> usize {
    hunks
        .lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
                || line.starts_with(' ')
        })
        .count()
}

/// Format staged revert file paths for the revert dock preview.
pub fn revert_files_preview(paths: &[String]) -> String {
    if paths.is_empty() {
        "No files to revert".to_string()
    } else {
        paths
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Pick the item with the lexicographically greatest timestamp key (RFC3339 works).
pub fn pick_latest_by_timestamp<T: Clone>(
    items: impl IntoIterator<Item = (T, String)>,
) -> Option<T> {
    items
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(item, _)| item)
}

/// First line of the earliest user message in a session, if any.
pub fn first_user_message_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .find_map(|m| {
            let text = m
                .parts
                .iter()
                .filter_map(|part| match part.kind {
                    PartKind::Text => part.text.clone(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        })
}

/// Derive a human-readable session label for sidebars and headers.
pub fn session_display_title(session: &Session, first_prompt: Option<&str>) -> String {
    if let Some(title) = session.title.as_ref() {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(prompt) = first_prompt {
        let one_line = prompt.lines().next().unwrap_or(prompt).trim();
        if !one_line.is_empty() {
            return truncate_preview(one_line, 40);
        }
    }
    "新会话".into()
}

/// Build session nav rows for the current project, newest first.
pub fn sessions_for_project(
    sessions: &[Session],
    running: &HashSet<SessionId>,
    first_prompts: &std::collections::HashMap<SessionId, String>,
) -> Vec<crate::layout::SessionNavItem> {
    let mut items: Vec<_> = sessions
        .iter()
        .map(|s| {
            let prompt = first_prompts.get(&s.id).map(String::as_str);
            crate::layout::SessionNavItem {
                id: s.id,
                title: session_display_title(s, prompt),
                updated_at: s.updated_at.to_rfc3339(),
                agent: s.agent,
                running: running.contains(&s.id),
            }
        })
        .collect();
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    items
}

pub fn extract_tool_path(tool_name: &str, input: &Value) -> Option<String> {
    let input = &deepseekcode_types::normalize_tool_arguments(input);
    match tool_name {
        "read" | "write" | "edit" => input
            .get("path")
            .and_then(|v| v.as_str())
            .map(String::from),
        _ => None,
    }
}

/// Whether an SSE event should trigger a full activity-tree projection.
pub fn event_affects_activity_tree(kind: EventKind) -> bool {
    deepseekcode_sdk::event_requires_full_rebuild(kind)
}

pub use deepseekcode_sdk::{
    patch_assistant_message_completed, patch_assistant_message_delta, patch_running_tool_leaf_preview,
    patch_tool_completed, patch_tool_started,
};

/// Planned right-panel route for an activity leaf click (unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityClickRoute {
    Editor {
        path: String,
        line: Option<u32>,
    },
    Terminal {
        detail_ref: String,
    },
    Tasks {
        todo_index: Option<usize>,
    },
    Files {
        path: Option<String>,
    },
    SwitchSession {
        session_id: String,
    },
    WebPreview {
        url: String,
        detail_ref: Option<String>,
    },
    ShowDetail,
    None,
}

pub fn plan_activity_click_route(
    kind: ActivityKind,
    target: &ActivityTargetKind,
    meta: &ActivityMeta,
    detail_ref: Option<&str>,
) -> ActivityClickRoute {
    match (kind, target) {
        (
            ActivityKind::Read
            | ActivityKind::Write
            | ActivityKind::Edit
            | ActivityKind::ApplyPatch,
            ActivityTargetKind::File {
                path,
                line_start,
                ..
            },
        ) => ActivityClickRoute::Editor {
            path: path.clone(),
            line: *line_start,
        },
        (ActivityKind::Grep, ActivityTargetKind::Pattern { .. }) => {
            if let (Some(path), Some(line)) = (
                meta.first_match_path.as_deref(),
                meta.first_match_line,
            ) {
                ActivityClickRoute::Editor {
                    path: path.to_string(),
                    line: Some(line),
                }
            } else {
                ActivityClickRoute::ShowDetail
            }
        }
        (ActivityKind::Lsp, _) => ActivityClickRoute::Editor {
            path: meta.first_match_path.clone().unwrap_or_default(),
            line: None,
        },
        (ActivityKind::Bash, ActivityTargetKind::Command { .. }) => detail_ref
            .map(|id| ActivityClickRoute::Terminal {
                detail_ref: id.to_string(),
            })
            .unwrap_or(ActivityClickRoute::ShowDetail),
        (ActivityKind::TodoWrite | ActivityKind::TodoRead, ActivityTargetKind::TodoList) => {
            ActivityClickRoute::Tasks {
                todo_index: meta.todo_focus_index.map(|i| i as usize),
            }
        }
        (ActivityKind::Task, ActivityTargetKind::Session { session_id, .. }) => {
            ActivityClickRoute::SwitchSession {
                session_id: session_id.clone(),
            }
        }
        (ActivityKind::WebFetch, ActivityTargetKind::Url { url })
        | (ActivityKind::WebSearch, ActivityTargetKind::Url { url }) => ActivityClickRoute::WebPreview {
            url: url.clone(),
            detail_ref: detail_ref.map(String::from),
        },
        (ActivityKind::Glob | ActivityKind::List, ActivityTargetKind::Pattern { .. }) => {
            ActivityClickRoute::Files {
                path: meta.first_match_path.clone(),
            }
        }
        (ActivityKind::Mcp, ActivityTargetKind::File { path, line_start, .. }) => {
            ActivityClickRoute::Editor {
                path: path.clone(),
                line: *line_start,
            }
        }
        (ActivityKind::Mcp, ActivityTargetKind::Url { url }) => ActivityClickRoute::WebPreview {
            url: url.clone(),
            detail_ref: detail_ref.map(String::from),
        },
        (ActivityKind::Mcp, _) => {
            if let Some(path) = meta
                .first_match_path
                .as_deref()
                .filter(|p| !p.is_empty())
            {
                ActivityClickRoute::Editor {
                    path: path.to_string(),
                    line: meta.first_match_line,
                }
            } else if let ActivityTargetKind::Generic { label } = target {
                if label.starts_with("http://") || label.starts_with("https://") {
                    ActivityClickRoute::WebPreview {
                        url: label.clone(),
                        detail_ref: detail_ref.map(String::from),
                    }
                } else if detail_ref.is_some() {
                    ActivityClickRoute::ShowDetail
                } else {
                    ActivityClickRoute::None
                }
            } else if detail_ref.is_some() {
                ActivityClickRoute::ShowDetail
            } else {
                ActivityClickRoute::None
            }
        }
        (ActivityKind::Question, _) => ActivityClickRoute::ShowDetail,
        (ActivityKind::PlanReady, _) => {
            if let Some(path) = meta.first_match_path.as_deref().filter(|p| !p.is_empty()) {
                ActivityClickRoute::Editor {
                    path: path.to_string(),
                    line: None,
                }
            } else {
                ActivityClickRoute::ShowDetail
            }
        }
        (ActivityKind::Permission, _) => ActivityClickRoute::ShowDetail,
        _ if detail_ref.is_some() => ActivityClickRoute::ShowDetail,
        _ => ActivityClickRoute::None,
    }
}

/// Primary todo row index encoded in a todo snapshot.
pub fn todo_focus_index_from_items(todos: &[TodoItem]) -> Option<usize> {
    todos
        .iter()
        .position(|t| t.status == "in_progress")
        .or_else(|| todos.iter().position(|t| t.status == "pending"))
}

/// Todo panel row to highlight when a todo activity leaf is selected.
pub fn todo_index_for_leaf(tree: &ActivityTree, leaf_id: &str) -> Option<usize> {
    let mut found = None;
    fn walk(nodes: &[ActivityNode], leaf_id: &str, found: &mut Option<usize>) {
        for node in nodes {
            match node {
                ActivityNode::Leaf(leaf)
                    if leaf.id == leaf_id
                        && matches!(leaf.kind, ActivityKind::TodoWrite | ActivityKind::TodoRead) =>
                {
                    *found = leaf.meta.todo_focus_index.map(|i| i as usize);
                }
                ActivityNode::Group(group) => walk(&group.children, leaf_id, found),
                _ => {}
            }
        }
    }
    walk(&tree.roots, leaf_id, &mut found);
    found
}

/// Activity leaf id to highlight when a todo panel row is selected.
pub fn find_todo_leaf_id_for_index(tree: &ActivityTree, index: usize) -> Option<String> {
    let mut match_id = None;
    let mut latest = None;
    fn walk(nodes: &[ActivityNode], index: usize, match_id: &mut Option<String>, latest: &mut Option<String>) {
        for node in nodes {
            match node {
                ActivityNode::Leaf(leaf)
                    if matches!(leaf.kind, ActivityKind::TodoWrite | ActivityKind::TodoRead) =>
                {
                    *latest = Some(leaf.id.clone());
                    if leaf.meta.todo_focus_index == Some(index as u32) {
                        *match_id = Some(leaf.id.clone());
                    }
                }
                ActivityNode::Group(group) => walk(&group.children, index, match_id, latest),
                _ => {}
            }
        }
    }
    walk(&tree.roots, index, &mut match_id, &mut latest);
    match_id.or(latest)
}

/// Compare hydrate API tree with locally projected tree (acceptance #9).
pub fn activity_trees_equivalent(a: &ActivityTree, b: &ActivityTree) -> bool {
    fn leaf_count(tree: &ActivityTree) -> usize {
        fn walk(nodes: &[ActivityNode], count: &mut usize) {
            for node in nodes {
                *count += 1;
                if let ActivityNode::Group(g) = node {
                    walk(&g.children, count);
                }
            }
        }
        let mut count = 0;
        walk(&tree.roots, &mut count);
        count
    }
    a.roots.len() == b.roots.len() && leaf_count(a) == leaf_count(b)
}

/// Whether an activity group should render expanded (user overrides + schema default).
pub fn is_activity_group_expanded(
    group: &deepseekcode_schema::activity::ActivityGroup,
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
) -> bool {
    if collapsed_groups.contains(&group.id) {
        return false;
    }
    if expanded_groups.contains(&group.id) {
        return true;
    }
    !group.collapsed_default
}

/// Toggle group expansion respecting `collapsed_default`.
pub fn toggle_activity_group_expansion(
    group_id: &str,
    collapsed_default: bool,
    expanded_groups: &mut HashSet<String>,
    collapsed_groups: &mut HashSet<String>,
) {
    let expanded = if collapsed_groups.contains(group_id) {
        false
    } else if expanded_groups.contains(group_id) {
        true
    } else {
        !collapsed_default
    };
    if expanded {
        collapsed_groups.insert(group_id.to_string());
        expanded_groups.remove(group_id);
    } else {
        collapsed_groups.remove(group_id);
        expanded_groups.insert(group_id.to_string());
    }
}

/// Ancestor `ActivityGroup` ids containing a leaf (for auto-expand on todo highlight).
pub fn ancestor_group_ids_for_leaf(tree: &ActivityTree, leaf_id: &str) -> Vec<String> {
    let mut path = Vec::new();
    fn walk(nodes: &[ActivityNode], leaf_id: &str, path: &mut Vec<String>) -> bool {
        for node in nodes {
            match node {
                ActivityNode::Group(group) => {
                    path.push(group.id.clone());
                    if walk(&group.children, leaf_id, path) {
                        return true;
                    }
                    path.pop();
                }
                ActivityNode::Leaf(leaf) if leaf.id == leaf_id => return true,
                ActivityNode::Leaf(_) => {}
            }
        }
        false
    }
    if walk(&tree.roots, leaf_id, &mut path) {
        path
    } else {
        Vec::new()
    }
}

pub const ACTIVITY_ROW_HEIGHT_PX: f32 = 28.0;

/// Approximate visible row index of a leaf for timeline scroll positioning.
pub fn activity_visible_row_index(
    tree: &ActivityTree,
    leaf_id: &str,
    expanded_groups: &HashSet<String>,
    collapsed_groups: &HashSet<String>,
    expanded_leaves: &HashSet<String>,
) -> Option<usize> {
    let mut row = 0usize;
    fn walk(
        nodes: &[ActivityNode],
        leaf_id: &str,
        expanded_groups: &HashSet<String>,
        collapsed_groups: &HashSet<String>,
        expanded_leaves: &HashSet<String>,
        row: &mut usize,
    ) -> bool {
        for node in nodes {
            match node {
                ActivityNode::Group(group) => {
                    *row += 1;
                    let expanded = is_activity_group_expanded(group, expanded_groups, collapsed_groups);
                    if expanded {
                        if group.summary.is_some() {
                            *row += 1;
                        }
                        if walk(
                            &group.children,
                            leaf_id,
                            expanded_groups,
                            collapsed_groups,
                            expanded_leaves,
                            row,
                        ) {
                            return true;
                        }
                    }
                }
                ActivityNode::Leaf(leaf) => {
                    if leaf.id == leaf_id {
                        return true;
                    }
                    *row += 1;
                    let message_expanded = expanded_leaves.contains(&leaf.id)
                        && leaf.text.is_some()
                        && matches!(
                            leaf.kind,
                            ActivityKind::Reasoning | ActivityKind::AssistantMessage
                        );
                    if message_expanded {
                        *row += 2;
                    }
                }
            }
        }
        false
    }
    if walk(
        &tree.roots,
        leaf_id,
        expanded_groups,
        collapsed_groups,
        expanded_leaves,
        &mut row,
    ) {
        Some(row)
    } else {
        None
    }
}

/// Index of the todo row to highlight when a todo activity leaf is selected.
pub fn default_todo_highlight_index(todos: &[TodoItem]) -> Option<usize> {
    todo_focus_index_from_items(todos).or_else(|| if todos.is_empty() { None } else { Some(0) })
}

/// Parse `git status --porcelain` lines into staged, unstaged, and untracked groups.
pub fn classify_git_porcelain(
    lines: &[String],
) -> (
    Vec<GitStatusEntry>,
    Vec<GitStatusEntry>,
    Vec<GitStatusEntry>,
) {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    for raw in lines {
        let line = raw.trim_end();
        if line.len() < 3 {
            continue;
        }
        if line.starts_with("?? ") {
            if let Some(path) = line.get(3..).map(str::trim).filter(|p| !p.is_empty()) {
                untracked.push(GitStatusEntry {
                    path: path.to_string(),
                    staged_label: String::new(),
                    unstaged_label: "Untracked".into(),
                    raw: raw.clone(),
                });
            }
            continue;
        }
        let index = line.as_bytes().first().copied().unwrap_or(b' ') as char;
        let worktree = line.as_bytes().get(1).copied().unwrap_or(b' ') as char;
        let path = line.get(3..).unwrap_or("").trim();
        if path.is_empty() {
            continue;
        }
        let path = path.split(" -> ").next().unwrap_or(path).trim().to_string();
        let entry = GitStatusEntry {
            path: path.clone(),
            staged_label: git_status_label(index, true),
            unstaged_label: git_status_label(worktree, false),
            raw: raw.clone(),
        };
        if index != ' ' && index != '?' {
            staged.push(entry.clone());
        }
        if worktree != ' ' && worktree != '?' {
            unstaged.push(entry);
        }
    }
    (staged, unstaged, untracked)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusEntry {
    pub path: String,
    pub staged_label: String,
    pub unstaged_label: String,
    pub raw: String,
}

fn git_status_label(code: char, staged: bool) -> String {
    match code {
        'M' => if staged { "Staged modified".into() } else { "Modified".into() },
        'A' => if staged { "Staged added".into() } else { "Added".into() },
        'D' => if staged { "Staged deleted".into() } else { "Deleted".into() },
        'R' => if staged { "Staged renamed".into() } else { "Renamed".into() },
        'C' => if staged { "Staged copied".into() } else { "Copied".into() },
        'U' => "Updated".into(),
        '?' => "Untracked".into(),
        _ => String::new(),
    }
}

/// Unified diff for a changed file (staged + unstaged) relative to HEAD.
pub fn git_unified_diff(project_dir: &str, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", project_dir, "diff", "HEAD", "--", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.is_empty() {
        let unstaged = std::process::Command::new("git")
            .args(["-C", project_dir, "diff", "--", path])
            .output()
            .ok()?;
        if unstaged.status.success() {
            return Some(String::from_utf8_lossy(&unstaged.stdout).to_string());
        }
    }
    Some(diff)
}

/// Read working-tree file content for git panel navigation.
pub fn git_worktree_file_content(project_dir: &str, path: &str) -> Option<String> {
    let full = std::path::Path::new(project_dir).join(path);
    std::fs::read_to_string(full).ok()
}

/// Most recent todo write/read leaf in the activity tree (last depth-first visit wins).
pub fn find_latest_todo_leaf_id(
    tree: &deepseekcode_schema::activity::ActivityTree,
) -> Option<String> {
    use deepseekcode_schema::activity::{ActivityKind, ActivityNode};
    let mut last = None;
    fn walk(nodes: &[ActivityNode], last: &mut Option<String>) {
        for node in nodes {
            match node {
                ActivityNode::Leaf(l)
                    if matches!(l.kind, ActivityKind::TodoWrite | ActivityKind::TodoRead) =>
                {
                    *last = Some(l.id.clone());
                }
                ActivityNode::Group(g) => walk(&g.children, last),
                _ => {}
            }
        }
    }
    walk(&tree.roots, &mut last);
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use serde_json::json;

    #[test]
    fn truncate_preview_respects_limit() {
        let long = "a".repeat(50);
        let out = truncate_preview(&long, 10);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 11);
    }

    #[test]
    fn token_usage_from_payload_reads_fields() {
        let payload = json!({
            "input_tokens": 1200,
            "output_tokens": 340,
            "reasoning_tokens": 80
        });
        assert_eq!(token_usage_from_payload(&payload), Some((1200, 340, 80)));
        assert!(token_usage_from_payload(&json!({})).is_none());
    }

    #[test]
    fn extract_write_path() {
        let path = extract_tool_path("write", &json!({"path": "src/main.rs"}));
        assert_eq!(path.as_deref(), Some("src/main.rs"));
        assert!(extract_tool_path("bash", &json!({"command": "ls"})).is_none());
    }

    #[test]
    fn unified_diff_row_count() {
        let hunks = "--- a\n+++ b\n@@\n-old\n+new\n context";
        assert_eq!(parse_unified_diff_rows(hunks), 3);
    }

    #[test]
    fn revert_files_preview_lists_paths() {
        let preview = revert_files_preview(&["src/main.rs".into(), "Cargo.toml".into()]);
        assert!(preview.contains("src/main.rs"));
        assert!(preview.contains("Cargo.toml"));
    }

    #[test]
    fn pick_latest_by_timestamp_selects_newest() {
        let picked = pick_latest_by_timestamp([
            ("old".to_string(), "2026-01-01T00:00:00Z".into()),
            ("new".to_string(), "2026-07-01T12:00:00Z".into()),
        ]);
        assert_eq!(picked.as_deref(), Some("new"));
    }

    #[test]
    fn sessions_for_project_sorts_newest_first() {
        use deepseekcode_schema::agent::AgentKind;
        use deepseekcode_schema::session::Session;
        use deepseekcode_types::{LocationId, SessionId};
        use chrono::Utc;

        let id_old = SessionId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let id_new = SessionId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let sessions = vec![
            Session {
                id: id_old,
                location_id: LocationId::new(),
                title: None,
                agent: AgentKind::Build,
                model_id: None,
                provider_id: None,
                parent_session_id: None,
                created_at: Utc::now(),
                updated_at: Utc::now() - chrono::Duration::days(1),
            },
            Session {
                id: id_new,
                location_id: LocationId::new(),
                title: Some("New chat".into()),
                agent: AgentKind::Build,
                model_id: None,
                provider_id: None,
                parent_session_id: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        ];
        let items = sessions_for_project(&sessions, &HashSet::new(), &std::collections::HashMap::new());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, id_new);
        assert_eq!(items[0].title, "New chat");
    }

    #[test]
    fn first_user_message_text_reads_earliest_user_row() {
        use deepseekcode_schema::message::{Message, MessagePart, MessageRole, PartKind};
        use deepseekcode_types::{MessageId, SessionId};

        let messages = vec![
            Message {
                id: MessageId::new(),
                session_id: SessionId::new(),
                role: MessageRole::Assistant,
                seq: 1,
                created_at: chrono::Utc::now(),
                parts: vec![MessagePart {
                    kind: PartKind::Text,
                    text: Some("hi".into()),
                    tool_call_id: None,
                    tool_name: None,
                    tool_input: None,
                    tool_output: None,
                }],
            },
            Message {
                id: MessageId::new(),
                session_id: SessionId::new(),
                role: MessageRole::User,
                seq: 2,
                created_at: chrono::Utc::now(),
                parts: vec![MessagePart {
                    kind: PartKind::Text,
                    text: Some("分析这个项目".into()),
                    tool_call_id: None,
                    tool_name: None,
                    tool_input: None,
                    tool_output: None,
                }],
            },
        ];
        assert_eq!(
            first_user_message_text(&messages).as_deref(),
            Some("分析这个项目")
        );
    }

    #[test]
    fn session_display_title_from_prompt() {
        use deepseekcode_schema::agent::AgentKind;
        use deepseekcode_schema::session::Session;
        use deepseekcode_types::{LocationId, SessionId};
        use chrono::Utc;

        let session = Session {
            id: SessionId::new(),
            location_id: LocationId::new(),
            title: None,
            agent: AgentKind::Build,
            model_id: None,
            provider_id: None,
            parent_session_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let title = session_display_title(&session, Some("Analyze the backend module please"));
        assert!(title.starts_with("Analyze"));
    }

    #[test]
    fn session_display_title_defaults_to_new_session_label() {
        use deepseekcode_schema::agent::AgentKind;
        use deepseekcode_schema::session::Session;
        use deepseekcode_types::{LocationId, SessionId};
        use chrono::Utc;

        let session = Session {
            id: SessionId::new(),
            location_id: LocationId::new(),
            title: None,
            agent: AgentKind::Build,
            model_id: None,
            provider_id: None,
            parent_session_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(session_display_title(&session, None), "新会话");
    }

    #[test]
    fn default_todo_highlight_index_prefers_in_progress() {
        use deepseekcode_schema::todo::TodoItem;
        let todos = vec![
            TodoItem {
                content: "a".into(),
                status: "pending".into(),
                priority: "medium".into(),
            },
            TodoItem {
                content: "b".into(),
                status: "in_progress".into(),
                priority: "high".into(),
            },
        ];
        assert_eq!(default_todo_highlight_index(&todos), Some(1));
    }

    #[test]
    fn plan_activity_click_routes_file_to_editor() {
        use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
        let route = plan_activity_click_route(
            ActivityKind::Read,
            &ActivityTargetKind::File {
                path: "src/main.rs".into(),
                line_start: Some(10),
                line_end: Some(17),
            },
            &ActivityMeta::default(),
            Some("tc-1"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Editor {
                path: "src/main.rs".into(),
                line: Some(10),
            }
        );
    }

    #[test]
    fn patch_assistant_message_delta_updates_running_leaf() {
        use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityNode, ActivityStatus, ActivityTargetKind, ActivityTree};
        let mut tree = ActivityTree::default();
        assert!(patch_assistant_message_delta(&mut tree, "msg-1", "Hello"));
        assert!(patch_assistant_message_delta(&mut tree, "msg-1", " world"));
        let ActivityNode::Leaf(leaf) = &tree.roots[0] else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.id, "msg-1");
        assert_eq!(leaf.text.as_deref(), Some("Hello world"));
        assert_eq!(leaf.status, ActivityStatus::Running);
        assert_eq!(leaf.kind, ActivityKind::AssistantMessage);
    }

    #[test]
    fn event_affects_activity_tree_excludes_message_delta() {
        assert!(!event_affects_activity_tree(EventKind::MessageDelta));
        assert!(!event_affects_activity_tree(EventKind::MessageCompleted));
        assert!(!event_affects_activity_tree(EventKind::ToolStarted));
        assert!(!event_affects_activity_tree(EventKind::ToolCompleted));
        assert!(event_affects_activity_tree(EventKind::SessionIdle));
    }

    #[test]
    fn plan_activity_click_routes_bash_to_terminal() {
        use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
        let route = plan_activity_click_route(
            ActivityKind::Bash,
            &ActivityTargetKind::Command {
                command: "npm test".into(),
            },
            &ActivityMeta::default(),
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
    fn plan_activity_click_routes_todo_with_focus_index() {
        use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
        let route = plan_activity_click_route(
            ActivityKind::TodoWrite,
            &ActivityTargetKind::TodoList,
            &ActivityMeta {
                todo_focus_index: Some(2),
                ..Default::default()
            },
            Some("todo-5"),
        );
        assert_eq!(
            route,
            ActivityClickRoute::Tasks {
                todo_index: Some(2)
            }
        );
    }

    #[test]
    fn event_affects_activity_tree_skips_tool_progress() {
        assert!(!event_affects_activity_tree(EventKind::ToolProgress));
        assert!(!event_affects_activity_tree(EventKind::ToolCompleted));
        assert!(event_affects_activity_tree(EventKind::ProviderTurnStarted));
    }

    #[test]
    fn patch_running_tool_leaf_preview_updates_leaf_text() {
        use deepseekcode_schema::activity::{
            ActivityKind, ActivityLeaf, ActivityNode, ActivityStatus, ActivityTargetKind,
            ActivityTree,
        };
        let mut tree = ActivityTree {
            roots: vec![ActivityNode::Leaf(ActivityLeaf {
                id: "bash-1".into(),
                kind: ActivityKind::Bash,
                verb: "Ran".into(),
                target: ActivityTargetKind::Command {
                    command: "npm install".into(),
                },
                meta: Default::default(),
                status: ActivityStatus::Running,
                detail_ref: Some("bash-1".into()),
                text: None,
            })],
        };
        assert!(patch_running_tool_leaf_preview(&mut tree, "bash-1", "[stdout] installing"));
        let ActivityNode::Leaf(leaf) = &tree.roots[0] else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.text.as_deref(), Some("[stdout] installing"));
    }

    #[test]
    fn find_todo_leaf_id_for_index_matches_focus() {
        use deepseekcode_schema::activity::{
            ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode, ActivityStatus,
            ActivityTargetKind, ActivityTree,
        };
        let tree = ActivityTree {
            roots: vec![
                ActivityNode::Leaf(ActivityLeaf {
                    id: "todo-1".into(),
                    kind: ActivityKind::TodoWrite,
                    verb: "Updated todos".into(),
                    target: ActivityTargetKind::TodoList,
                    meta: ActivityMeta {
                        todo_focus_index: Some(0),
                        ..Default::default()
                    },
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: None,
                }),
                ActivityNode::Leaf(ActivityLeaf {
                    id: "todo-9".into(),
                    kind: ActivityKind::TodoWrite,
                    verb: "Updated todos".into(),
                    target: ActivityTargetKind::TodoList,
                    meta: ActivityMeta {
                        todo_focus_index: Some(2),
                        ..Default::default()
                    },
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: None,
                }),
            ],
        };
        assert_eq!(find_todo_leaf_id_for_index(&tree, 2).as_deref(), Some("todo-9"));
        assert_eq!(todo_index_for_leaf(&tree, "todo-9"), Some(2));
    }

    #[test]
    fn classify_git_porcelain_groups_staged_and_unstaged() {
        let lines = vec![
            "M  staged.rs".to_string(),
            " M unstaged.rs".to_string(),
            "?? new.rs".to_string(),
        ];
        let (staged, unstaged, untracked) = classify_git_porcelain(&lines);
        assert_eq!(staged.len(), 1);
        assert_eq!(unstaged.len(), 1);
        assert_eq!(untracked.len(), 1);
        assert_eq!(staged[0].path, "staged.rs");
        assert_eq!(unstaged[0].path, "unstaged.rs");
        assert_eq!(untracked[0].path, "new.rs");
    }

    #[test]
    fn find_latest_todo_leaf_prefers_last() {
        use deepseekcode_schema::activity::{
            ActivityKind, ActivityLeaf, ActivityNode, ActivityStatus, ActivityTargetKind,
            ActivityTree,
        };
        let tree = ActivityTree {
            roots: vec![
                ActivityNode::Leaf(ActivityLeaf {
                    id: "todo-1".into(),
                    kind: ActivityKind::TodoWrite,
                    verb: "Updated todos".into(),
                    target: ActivityTargetKind::TodoList,
                    meta: Default::default(),
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: None,
                }),
                ActivityNode::Leaf(ActivityLeaf {
                    id: "todo-9".into(),
                    kind: ActivityKind::TodoWrite,
                    verb: "Updated todos".into(),
                    target: ActivityTargetKind::TodoList,
                    meta: Default::default(),
                    status: ActivityStatus::Completed,
                    detail_ref: None,
                    text: None,
                }),
            ],
        };
        assert_eq!(find_latest_todo_leaf_id(&tree).as_deref(), Some("todo-9"));
    }

    #[test]
    fn implement_prompt_from_plan_path_matches_spec() {
        let prompt = implement_prompt_from_plan_path(".deepseekcode/plan/foo/PLAN.md");
        assert!(prompt.contains(".deepseekcode/plan/foo/PLAN.md"));
        assert!(prompt.contains("第一步"));
    }

    #[test]
    fn agent_selector_for_kind_maps_plan_build_general() {
        assert_eq!(agent_selector_for_kind(AgentKind::Plan), "plan");
        assert_eq!(agent_selector_for_kind(AgentKind::Build), "build");
        assert_eq!(agent_selector_for_kind(AgentKind::General), "general");
    }

    #[test]
    fn plan_ready_activity_click_routes_to_editor() {
        use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
        let mut meta = ActivityMeta::default();
        meta.first_match_path = Some(".deepseekcode/plan/x/PLAN.md".into());
        let route = plan_activity_click_route(
            ActivityKind::PlanReady,
            &ActivityTargetKind::Generic {
                label: "Plan ready".into(),
            },
            &meta,
            None,
        );
        assert_eq!(
            route,
            ActivityClickRoute::Editor {
                path: ".deepseekcode/plan/x/PLAN.md".into(),
                line: None,
            }
        );
    }
}
