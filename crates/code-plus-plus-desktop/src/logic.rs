//! Pure logic helpers for desktop (unit-testable without GPUI).
//!
//! No engine/SDK I/O lives here, so this module emits no `tracing!` sites.
//! Product logging for session/prompt/engine failures (DESKTOP target with
//! `run_id` + `session_id`) lives in `workspace.rs` + `session_activity.rs`;
//! timeline render diagnostics live in `activity_timeline.rs`.

use code_plus_plus_schema::activity::{
    ActivityGroupKind, ActivityKind, ActivityMeta, ActivityNode, ActivityTargetKind, ActivityTree,
};
use code_plus_plus_schema::agent::AgentKind;
use code_plus_plus_schema::event::EventKind;
use code_plus_plus_schema::message::{Message, MessageRole, PartKind};
use code_plus_plus_schema::session::Session;
use code_plus_plus_schema::todo::TodoItem;
use code_plus_plus_types::SessionId;
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
pub fn latest_context_usage_from_events(
    events: &[code_plus_plus_schema::event::EventEnvelope],
) -> Option<(u32, u32, u32)> {
    use code_plus_plus_schema::event::EventKind;
    events.iter().rev().find_map(|event| {
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
    let input = &code_plus_plus_types::normalize_tool_arguments(input);
    match tool_name {
        "read" | "write" | "edit" => input.get("path").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    }
}

/// Whether an SSE event should trigger a full activity-tree projection.
pub fn event_affects_activity_tree(kind: EventKind) -> bool {
    code_plus_plus_sdk::event_requires_full_rebuild(kind)
}

pub use code_plus_plus_sdk::{
    patch_assistant_message_completed, patch_assistant_message_delta,
    patch_running_tool_leaf_preview, patch_tool_completed, patch_tool_started,
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
                path, line_start, ..
            },
        ) => ActivityClickRoute::Editor {
            path: path.clone(),
            line: *line_start,
        },
        (ActivityKind::Grep, ActivityTargetKind::Pattern { .. }) => {
            if let (Some(path), Some(line)) =
                (meta.first_match_path.as_deref(), meta.first_match_line)
            {
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
        | (ActivityKind::WebSearch, ActivityTargetKind::Url { url }) => {
            ActivityClickRoute::WebPreview {
                url: url.clone(),
                detail_ref: detail_ref.map(String::from),
            }
        }
        (ActivityKind::Glob | ActivityKind::List, ActivityTargetKind::Pattern { .. }) => {
            ActivityClickRoute::Files {
                path: meta.first_match_path.clone(),
            }
        }
        (
            ActivityKind::Mcp,
            ActivityTargetKind::File {
                path, line_start, ..
            },
        ) => ActivityClickRoute::Editor {
            path: path.clone(),
            line: *line_start,
        },
        (ActivityKind::Mcp, ActivityTargetKind::Url { url }) => ActivityClickRoute::WebPreview {
            url: url.clone(),
            detail_ref: detail_ref.map(String::from),
        },
        (ActivityKind::Mcp, _) => {
            if let Some(path) = meta.first_match_path.as_deref().filter(|p| !p.is_empty()) {
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
                        && matches!(
                            leaf.kind,
                            ActivityKind::TodoWrite | ActivityKind::TodoRead
                        ) =>
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
    fn walk(
        nodes: &[ActivityNode],
        index: usize,
        match_id: &mut Option<String>,
        latest: &mut Option<String>,
    ) {
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
    group: &code_plus_plus_schema::activity::ActivityGroup,
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
/// Walks the orphan-adopted tree so adopted leaves report their display parent.
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
    let owned = crate::activity_rows::adopt_orphans_for_display(tree);
    if walk(&owned, leaf_id, &mut path) {
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
                    let expanded =
                        is_activity_group_expanded(group, expanded_groups, collapsed_groups);
                    if expanded {
                        // Mirrors flatten: turn summaries merge into the header.
                        if group.summary.is_some() && group.kind != ActivityGroupKind::ProviderTurn
                        {
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
    // Same display adoption as render (stale + orphans), so the computed
    // row index matches the displayed rows.
    let owned = crate::activity_rows::adopt_orphans_for_display(tree);
    if walk(
        &owned,
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
        // NOTE: converted porcelain-v2 lines use `.` for the unmodified
        // side (classic porcelain only ever emits `' '`). Both spellings
        // mean "nothing to show here" — without this, a `.M` worktree-only
        // edit lands in Staged AND Unstaged at once (caught by live UI test).
        if index != ' ' && index != '.' && index != '?' {
            staged.push(entry.clone());
        }
        if worktree != ' ' && worktree != '.' && worktree != '?' {
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
        'M' => {
            if staged {
                code_plus_plus_i18n::t("git.status.staged_modified")
            } else {
                code_plus_plus_i18n::t("git.status.modified")
            }
        }
        'A' => {
            if staged {
                code_plus_plus_i18n::t("git.status.staged_added")
            } else {
                code_plus_plus_i18n::t("git.status.added")
            }
        }
        'D' => {
            if staged {
                code_plus_plus_i18n::t("git.status.staged_deleted")
            } else {
                code_plus_plus_i18n::t("git.status.deleted")
            }
        }
        'R' => {
            if staged {
                code_plus_plus_i18n::t("git.status.staged_renamed")
            } else {
                code_plus_plus_i18n::t("git.status.renamed")
            }
        }
        'C' => {
            if staged {
                code_plus_plus_i18n::t("git.status.staged_copied")
            } else {
                code_plus_plus_i18n::t("git.status.copied")
            }
        }
        'U' => code_plus_plus_i18n::t("git.status.updated"),
        '?' => code_plus_plus_i18n::t("git.status.untracked"),
        _ => String::new(),
    }
}

/// Unified diff for a changed file (staged + unstaged) relative to HEAD.
pub fn git_unified_diff(project_dir: &str, path: &str) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    code_plus_plus_core::process::hide_console(&mut cmd);
    let output = cmd
        .args(["-C", project_dir, "diff", "HEAD", "--", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.is_empty() {
        let mut unstaged_cmd = std::process::Command::new("git");
        code_plus_plus_core::process::hide_console(&mut unstaged_cmd);
        let unstaged = unstaged_cmd
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

// ---------------------------------------------------------------------------
// Cursor-parity Git models (P0-P5). Pure + unit-tested; no git spawning here.
// ---------------------------------------------------------------------------

/// Branch row from `git branch -vv --all`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranch {
    pub name: String,
    pub is_remote: bool,
    pub is_head: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub last_commit: String,
}

/// Commit node from `log --pretty=format:%H%x1f%P%x1f%an%x1f%ct%x1f%s%x1f%D`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommitNode {
    pub sha: String,
    pub short_sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub timestamp: i64,
    pub subject: String,
    pub decorations: Vec<String>,
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    pub is_head: bool,
    pub lane: usize,
}

/// Stash row from `stash list --pretty=format:%gd%x1f%H%x1f%s%x1f%ct`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStashEntry {
    pub ref_name: String,
    pub sha: String,
    pub subject: String,
    pub timestamp: i64,
}

/// Remote row from `remote -v` (fetch/push collapsed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// Agent-review finding rendered under AGENT REVIEW.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitReviewIssue {
    pub path: String,
    pub line: Option<u32>,
    pub severity: String,
    pub message: String,
}

/// Parse `git branch -vv --all --no-color` lines.
pub fn parse_branch_vv(output: &str) -> Vec<GitBranch> {
    let mut out = Vec::new();
    for raw in output.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        // `* main abc1234 [origin/main: ahead 1] subject` or `  remotes/origin/x ...`
        let is_head = line.starts_with('*');
        let body = line.trim_start_matches('*').trim_start_matches(' ').trim();
        let mut parts = body.split_whitespace();
        let Some(name_raw) = parts.next() else {
            continue;
        };
        let is_remote = name_raw.starts_with("remotes/");
        let name = if is_remote {
            name_raw
                .strip_prefix("remotes/")
                .unwrap_or(name_raw)
                .to_string()
        } else {
            name_raw.to_string()
        };
        // Skip sha token.
        let _sha = parts.next().unwrap_or("");
        // Remainder may start with `[upstream...]` then subject.
        let rest = body
            .splitn(3, char::is_whitespace)
            .nth(2)
            .unwrap_or("")
            .trim();
        let (upstream, ahead, behind, last_commit) = parse_branch_remainder(rest);
        out.push(GitBranch {
            name,
            is_remote,
            is_head,
            upstream,
            ahead,
            behind,
            last_commit,
        });
    }
    out.sort_by(|a, b| {
        // HEAD first, then local, then remote, each alphabetical.
        (b.is_head, !b.is_remote, &a.name).cmp(&(a.is_head, !a.is_remote, &b.name))
    });
    out
}

fn parse_branch_remainder(rest: &str) -> (Option<String>, u32, u32, String) {
    if let Some(bracket_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
        let inside = &rest[1..1 + bracket_end];
        let after = rest[1 + bracket_end + 1..].trim().to_string();
        // inside: `origin/main`, `origin/main: ahead 1`, `gone`, ...
        let (upstream_part, ahead, behind) = if inside == "gone" {
            (None, 0, 0)
        } else {
            let (up, trail) = match inside.split_once(':') {
                Some((u, t)) => (u.trim(), t.trim()),
                None => (inside.trim(), ""),
            };
            let mut ahead = 0u32;
            let mut behind = 0u32;
            for seg in trail.split(',') {
                let seg = seg.trim();
                if let Some(n) = seg.strip_prefix("ahead ") {
                    ahead = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = seg.strip_prefix("behind ") {
                    behind = n.trim().parse().unwrap_or(0);
                }
            }
            (
                if up.is_empty() {
                    None
                } else {
                    Some(up.to_string())
                },
                ahead,
                behind,
            )
        };
        (upstream_part, ahead, behind, after)
    } else {
        (None, 0, 0, rest.to_string())
    }
}

/// Parse machine-readable log lines (`\x1f` separated, 6 fields).
pub fn parse_log_pretty(output: &str, head_sha: Option<&str>) -> Vec<GitCommitNode> {
    let mut out = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\x1f').collect();
        if fields.len() < 6 {
            continue;
        }
        let sha = fields[0].to_string();
        let parents: Vec<String> = fields[1]
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let author = fields[2].to_string();
        let timestamp = fields[3].parse().unwrap_or(0);
        let subject = fields[4].to_string();
        let decor_raw = fields[5];
        let mut decorations = Vec::new();
        let mut branches = Vec::new();
        let mut remotes = Vec::new();
        let mut is_head = false;
        for d in decor_raw.split(", ") {
            let d = d.trim();
            if d.is_empty() {
                continue;
            }
            // Decorations look like `HEAD -> main, origin/main, tag: v1`.
            let d = d
                .strip_prefix("HEAD -> ")
                .map(|s| {
                    is_head = true;
                    s
                })
                .unwrap_or(d);
            if d == "HEAD" {
                is_head = true;
            }
            decorations.push(d.to_string());
            if d.starts_with("tag: ") {
                continue;
            }
            if d == "HEAD" {
                continue;
            }
            // Remote-tracking refs are `origin/<branch>` / `upstream/<branch>`;
            // local branches may themselves contain `/` (e.g. `upgrade/sdk-54`),
            // so only the well-known remote prefixes count as remotes.
            if d.starts_with("origin/") || d.starts_with("upstream/") || d.starts_with("remotes/") {
                remotes.push(d.to_string());
            } else {
                branches.push(d.to_string());
            }
        }
        if let Some(head) = head_sha {
            if sha == head {
                is_head = true;
            }
        }
        let short_sha = sha.chars().take(7).collect();
        out.push(GitCommitNode {
            sha,
            short_sha,
            parents,
            author,
            timestamp,
            subject,
            decorations,
            branches,
            remotes,
            is_head,
            lane: 0,
        });
    }
    assign_graph_lanes(&mut out);
    out
}

/// Minimal lane assignment so the GRAPH column draws stable vertical rails:
/// first-parent stays on its lane, new parents open the next free lane.
pub fn assign_graph_lanes(commits: &mut [GitCommitNode]) {
    use std::collections::HashMap;
    let mut sha_lane: HashMap<String, usize> = HashMap::new();
    let mut next_lane = 0usize;
    for c in commits.iter_mut() {
        let lane = sha_lane.remove(&c.sha).unwrap_or_else(|| {
            let l = next_lane;
            next_lane += 1;
            l
        });
        c.lane = lane;
        for (i, p) in c.parents.iter().enumerate() {
            if i == 0 {
                sha_lane.entry(p.clone()).or_insert(lane);
            } else if !sha_lane.contains_key(p) {
                let l = next_lane;
                next_lane += 1;
                sha_lane.insert(p.clone(), l);
            }
        }
    }
}

/// Parse `stash list --pretty=format:%gd%x1f%H%x1f%s%x1f%ct`.
pub fn parse_stash_list(output: &str) -> Vec<GitStashEntry> {
    let mut out = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 4 {
            continue;
        }
        out.push(GitStashEntry {
            ref_name: f[0].to_string(),
            sha: f[1].to_string(),
            subject: f[2].to_string(),
            timestamp: f[3].parse().unwrap_or(0),
        });
    }
    out
}

/// Parse `git remote -v` into collapsed remotes.
pub fn parse_remotes(output: &str) -> Vec<GitRemote> {
    use std::collections::HashMap;
    let mut map: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for line in output.lines() {
        // `origin\tgit@github.com:x/y.git (fetch)`
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let entry = map.entry(name.to_string()).or_insert((None, None));
        if kind.contains("fetch") {
            entry.0 = Some(url.to_string());
        } else if kind.contains("push") {
            entry.1 = Some(url.to_string());
        }
    }
    let mut out: Vec<GitRemote> = map
        .into_iter()
        .map(|(name, (fetch, push))| GitRemote {
            fetch_url: fetch.clone().unwrap_or_default(),
            push_url: push.clone().unwrap_or_else(|| fetch.unwrap_or_default()),
            name,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse `rev-list --left-right --count HEAD...@{u}` output ("1\t2").
pub fn parse_ahead_behind(output: &str) -> (u32, u32) {
    let nums: Vec<u32> = output
        .split(|c: char| c.is_whitespace() || c == '\t')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums.as_slice() {
        [a, b, ..] => (*a, *b),
        [a] => (*a, 0),
        _ => (0, 0),
    }
}

/// Case-insensitive substring filter for the Changes search box.
pub fn filter_changes(entries: &[GitStatusEntry], query: &str) -> Vec<GitStatusEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|e| e.path.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Whether a path looks like a merge conflict (`UU`/`AA`/`DD` etc.).
pub fn is_conflicted_raw(raw: &str) -> bool {
    let mut chars = raw.chars();
    let (a, b) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));
    matches!((a, b), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'))
}

/// Split conflicted entries out of staged/unstaged for the Merge Changes group.
pub fn partition_conflicts(
    staged: Vec<GitStatusEntry>,
    unstaged: Vec<GitStatusEntry>,
) -> (
    Vec<GitStatusEntry>,
    Vec<GitStatusEntry>,
    Vec<GitStatusEntry>,
) {
    let mut clean_staged = Vec::new();
    let mut clean_unstaged = Vec::new();
    let mut conflicted = Vec::new();
    for e in staged {
        if is_conflicted_raw(&e.raw) {
            if !conflicted.iter().any(|c: &GitStatusEntry| c.path == e.path) {
                conflicted.push(e);
            }
        } else {
            clean_staged.push(e);
        }
    }
    for e in unstaged {
        if is_conflicted_raw(&e.raw) {
            if !conflicted.iter().any(|c: &GitStatusEntry| c.path == e.path) {
                conflicted.push(e);
            }
        } else {
            clean_unstaged.push(e);
        }
    }
    (clean_staged, clean_unstaged, conflicted)
}

/// Commit box placeholder: `Message (Ctrl+Enter to commit on "branch")`.
pub fn commit_placeholder(branch: Option<&str>) -> String {
    match branch {
        Some(b) if !b.trim().is_empty() => {
            let short: String = b.chars().take(12).collect();
            format!("Message (Ctrl+Enter to commit on \"{short}\")")
        }
        _ => "Message (Ctrl+Enter to commit)".to_string(),
    }
}

/// Empty-commit validation for the Commit button enabled state.
pub fn commit_message_error(message: &str) -> Option<String> {
    if message.trim().is_empty() {
        Some("commit message is empty".to_string())
    } else {
        None
    }
}

/// Split a unified diff into per-hunk patches (each keeps file headers).
pub fn split_diff_hunks(diff: &str) -> Vec<String> {
    let mut header: Vec<String> = Vec::new();
    let mut hunks: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in diff.lines() {
        if line.starts_with("@@") {
            if let Some(c) = current.take() {
                hunks.push(c);
            }
            current = Some(Vec::new());
            if let Some(c) = current.as_mut() {
                c.push(line.to_string());
            }
        } else if current.is_some() {
            if let Some(c) = current.as_mut() {
                c.push(line.to_string());
            }
        } else if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file")
            || line.starts_with("deleted ")
            || line.starts_with("Binary ")
        {
            header.push(line.to_string());
        }
    }
    if let Some(c) = current.take() {
        hunks.push(c);
    }
    hunks
        .into_iter()
        .map(|h| {
            let mut out = header.clone();
            out.extend(h);
            out.join("\n")
        })
        .collect()
}

/// Review scope label for AGENT REVIEW (`Review diffs vs. {scope}.`).
pub fn review_scope_label(scope: &str) -> String {
    format!("Review diffs vs. {scope}.")
}

/// Most recent todo write/read leaf in the activity tree (last depth-first visit wins).
pub fn find_latest_todo_leaf_id(
    tree: &code_plus_plus_schema::activity::ActivityTree,
) -> Option<String> {
    use code_plus_plus_schema::activity::{ActivityKind, ActivityNode};
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

/// Compact one-line preview of a text payload for a permission summary.
///
/// Keeps the first non-empty line so a multi-line file body collapses to
/// something recognizable instead of dumping hundreds of characters.
fn first_line_preview(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    truncate_preview(line, max_chars)
}

/// Line/byte size of a text payload, e.g. `42 lines · 1.8 KB`.
fn size_summary(text: &str) -> String {
    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    let bytes = text.len() as u64;
    // Fixed bug: the old helper divided by 1024 twice and still printed `KB`.
    format!(
        "{} · {}",
        code_plus_plus_i18n::tr!("tool.detail.line_count", "count" => lines as u64),
        code_plus_plus_i18n::bytes(bytes)
    )
}

/// Files touched by a unified diff, in order, de-duplicated.
///
/// A `--- a/x` / `+++ b/x` pair names ONE file, so the two header lines are
/// paired: the new path wins, and the old path is used only for deletions
/// (`+++ /dev/null`). Matching either side independently reported rename
/// pairs as two files (`old.txt` and `new.txt`).
fn patch_targets(patch: &str) -> Vec<String> {
    fn clean(raw: &str) -> Option<String> {
        let path = raw.split('\t').next().unwrap_or("").trim();
        let path = path
            .strip_prefix("b/")
            .or_else(|| path.strip_prefix("a/"))
            .unwrap_or(path);
        if path.is_empty() || path == "/dev/null" {
            None
        } else {
            Some(path.to_string())
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut pending_old: Option<String> = None;
    let add = |out: &mut Vec<String>, path: String| {
        if !out.iter().any(|p| *p == path) {
            out.push(path);
        }
    };
    for line in patch.lines() {
        if let Some(raw) = line.strip_prefix("--- ") {
            pending_old = clean(raw);
        } else if let Some(raw) = line.strip_prefix("+++ ") {
            // New path when present, else the old path (pure deletion).
            let path = clean(raw).or_else(|| pending_old.clone());
            if let Some(path) = path {
                add(&mut out, path);
            }
            pending_old = None;
        }
    }
    out
}

fn hunk_count(patch: &str) -> usize {
    patch.lines().filter(|l| l.starts_with("@@")).count()
}

/// Concise, decision-relevant summary of a permission request.
///
/// The permission dock asks one question — "may this run?" — so it must show
/// only the facts needed to answer: what action, on which target, changing
/// what. It must NOT dump the tool payload: a `write`/`edit` of a real source
/// file carries hundreds to thousands of characters of file content, which
/// buried the actual request (and pushed the decision buttons off-screen).
///
/// Full tool input remains available in the activity detail panel
/// ([`format_tool_input`]) for deliberate drill-down.
pub fn summarize_permission_request(tool: &str, input: &Value) -> String {
    let input = &code_plus_plus_types::normalize_tool_arguments(input);
    let str_field = |key: &str| input[key].as_str().unwrap_or("").to_string();
    let path = str_field("path");
    let mut lines: Vec<String> = Vec::new();
    let mut push = |line: String| lines.push(line);

    match tool {
        "bash" => {
            let command = str_field("command");
            // The command IS the decision; show it in full but bounded.
            push(truncate_preview(command.trim(), 400));
        }
        "read" | "list" => {
            if !path.is_empty() {
                push(code_plus_plus_i18n::tr!("tool.detail.list", "path" => path.as_str()));
            }
        }
        "write" => {
            let content = str_field("content");
            if path.is_empty() && content.is_empty() {
                // Fall through to the generic empty-payload handling below.
            } else {
                push(code_plus_plus_i18n::tr!(
                    "tool.detail.write",
                    "path" => path.as_str(),
                    "content" => size_summary(&content),
                    "first_line" => first_line_preview(&content, 80)
                ));
            }
        }
        "edit" => {
            let old = str_field("old_string");
            let new = str_field("new_string");
            if !old.is_empty() || !new.is_empty() {
                let old_lines = if old.is_empty() {
                    0
                } else {
                    old.lines().count()
                };
                let new_lines = if new.is_empty() {
                    0
                } else {
                    new.lines().count()
                };
                push(code_plus_plus_i18n::tr!(
                    "tool.detail.edit",
                    "path" => path.as_str(),
                    "old_lines" => old_lines as u64,
                    "new_lines" => new_lines as u64,
                    "matched" => first_line_preview(&old, 80),
                    "inserted" => first_line_preview(&new, 80)
                ));
            } else if !path.is_empty() {
                push(code_plus_plus_i18n::tr!("tool.detail.list", "path" => path.as_str()));
            }
        }
        "apply_patch" => {
            let patch = str_field("patch");
            let targets = patch_targets(&patch);
            let hunks = hunk_count(&patch);
            if targets.is_empty() {
                push(code_plus_plus_i18n::tr!(
                    "tool.detail.apply_patch_hunks",
                    "hunks" => hunks as u64,
                    "preview" => size_summary(&patch)
                ));
            } else {
                let listed: Vec<String> = targets
                    .iter()
                    .take(6)
                    .map(|target| {
                        code_plus_plus_i18n::tr!(
                            "tool.detail.apply_patch_target",
                            "target" => target.as_str()
                        )
                    })
                    .collect();
                let mut body = code_plus_plus_i18n::tr!(
                    "tool.detail.apply_patch_files",
                    "files" => targets.len() as u64,
                    "hunks" => hunks as u64,
                    "targets" => listed.join("\n")
                );
                if targets.len() > 6 {
                    body.push('\n');
                    body.push_str(&code_plus_plus_i18n::tr!(
                        "tool.detail.apply_patch_more",
                        "count" => (targets.len() - 6) as u64
                    ));
                }
                push(body);
            }
        }
        "grep" => {
            push(code_plus_plus_i18n::tr!(
                "tool.detail.grep",
                "pattern" => truncate_preview(&str_field("pattern"), 120),
                "path" => if path.is_empty() { ".".to_string() } else { path.clone() }
            ));
        }
        "glob" => {
            let pattern = input["pattern"]
                .as_str()
                .or_else(|| input["glob_pattern"].as_str())
                .unwrap_or("*");
            push(code_plus_plus_i18n::tr!("tool.detail.glob", "pattern" => pattern));
        }
        "webfetch" => push(code_plus_plus_i18n::tr!(
            "tool.detail.webfetch",
            "url" => truncate_preview(&str_field("url"), 200)
        )),
        "websearch" => push(code_plus_plus_i18n::tr!(
            "tool.detail.websearch",
            "query" => truncate_preview(&str_field("query"), 120)
        )),
        "skill" => push(code_plus_plus_i18n::tr!(
            "tool.detail.skill",
            "name" => truncate_preview(&str_field("name"), 80)
        )),
        "task" => {
            push(code_plus_plus_i18n::tr!(
                "tool.detail.task",
                "agent" => str_field("agent"),
                "prompt" => first_line_preview(&str_field("prompt"), 100)
            ));
        }
        "question" => {
            let question = input["question"]
                .as_str()
                .or_else(|| input["prompt"].as_str())
                .unwrap_or("");
            push(first_line_preview(question, 200));
        }
        "plan_complete" | "plan.complete" => {
            push(code_plus_plus_i18n::tr!(
                "tool.detail.plan_complete",
                "title" => truncate_preview(&str_field("title"), 100)
            ));
        }
        "todowrite" | "todo_write" => {
            let todos = input["todos"].as_array().cloned().unwrap_or_default();
            let total = todos.len();
            let done = todos
                .iter()
                .filter(|t| {
                    t.get("status")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case("completed") || s == "done")
                        .unwrap_or(false)
                })
                .count();
            push(code_plus_plus_i18n::tr!(
                "tool.detail.todo_write",
                "total" => total as u64,
                "done" => done as u64,
                "first" => todos
                    .first()
                    .and_then(|t| t.get("content"))
                    .and_then(|c| c.as_str())
                    .map(|first| truncate_preview(first, 80))
                    .unwrap_or_default()
            ));
        }
        "todoread" | "todo_read" => push(code_plus_plus_i18n::t("tool.detail.todo_read")),
        "lsp" => push(code_plus_plus_i18n::tr!(
            "tool.detail.lsp",
            "operation" => truncate_preview(&str_field("operation"), 80)
        )),
        _ => {
            // An empty payload carries no information; printing `{}` (or a
            // long blob) would just add noise to the decision prompt.
            let is_empty = match input {
                Value::Null => true,
                Value::Object(map) => map.is_empty(),
                _ => false,
            };
            if !is_empty {
                let pretty =
                    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                // Unknown tools stay inspectable, but never unbounded.
                push(truncate_preview(pretty.trim(), 300));
            }
        }
    }

    if lines.is_empty() {
        lines.push(code_plus_plus_i18n::t("tool.detail.unknown"));
    }
    lines.join("\n")
}

/// What the composer's footer button does when clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    /// Abort the running task.
    Stop,
    /// Submit the prompt.
    Send,
    /// Nothing — a permission/question prompt owns the composer.
    None,
}

/// Resolve the composer footer button action.
///
/// The invariant that matters: **stop is never gated on `input_blocked`.** A
/// run parked on a permission or question prompt is precisely when the user
/// needs to abort, yet the guard used to swallow the click first, leaving an
/// inert square (greyed out, `on_mouse_down` returning immediately) while the
/// task kept running. Gating applies to *send* only.
pub fn composer_button_action(
    session_busy: bool,
    input_blocked: bool,
    stopping: bool,
) -> ComposerAction {
    if stopping {
        // P0-2: Stop in flight — disable the button so a rapid second click
        // cannot flip into Send and mis-fire a prompt.
        ComposerAction::None
    } else if session_busy {
        ComposerAction::Stop
    } else if input_blocked {
        ComposerAction::None
    } else {
        ComposerAction::Send
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn truncate_preview_respects_limit() {
        let long = "a".repeat(50);
        let out = truncate_preview(&long, 10);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 11);
    }

    #[test]
    fn permission_summary_never_dumps_write_content() {
        // 截图问题：edit/write 把整个文件内容（800+ 字符）倒进权限框，
        // 把决策按钮挤出可视区。用户只需要知道「改哪个文件、改成什么」。
        let long_body = format!("fn assess() {{\n{}\n}}", "x".repeat(2000));
        let out = summarize_permission_request(
            "edit",
            &json!({
                "path": "packages/agent-core/src/tools/assess-response.ts",
                "old_string": "function normalizeWord(text: str",
                "new_string": long_body,
            }),
        );
        assert!(out.contains("path: packages/agent-core/src/tools/assess-response.ts"));
        assert!(out.contains("replace 1 line with"));
        assert!(!out.contains("xxxx"), "file body leaked into the prompt");
        // 短小、几行内可读完。
        assert!(out.lines().count() <= 7, "{out}");
        assert!(out.chars().count() < 500, "summary too long: {}", out.len());
    }

    #[test]
    fn permission_summary_write_reports_size_not_content() {
        let content = "line one\nline two\nline three\n".repeat(50);
        let out = summarize_permission_request(
            "write",
            &json!({ "path": "src/a.ts", "content": content }),
        );
        assert!(out.contains("path: src/a.ts"));
        assert!(out.contains("150 lines ·"));
        assert!(out.contains("first line: line one"));
        assert!(!out.contains("line three\nline one"), "content leaked");
    }

    #[test]
    fn permission_summary_apply_patch_lists_files_only() {
        let patch = "--- a/old.txt\n+++ b/new.txt\n@@ -1,2 +1,3 @@\n a\n+b\n--- a/old.txt\n+++ b/second.txt\n@@ -5 +5 @@\n-c\n+d\n";
        let out = summarize_permission_request("apply_patch", &json!({ "patch": patch }));
        assert!(out.contains("2 files"), "{out}");
        assert!(out.contains("2 hunks"), "{out}");
        assert!(out.contains("- new.txt"), "{out}");
        assert!(out.contains("- second.txt"), "{out}");
        assert!(!out.contains("@@"), "diff body leaked: {out}");
    }

    #[test]
    fn permission_summary_keeps_bash_command_visible() {
        // 命令本身就是决策依据，必须完整可见。
        let out = summarize_permission_request("bash", &json!({ "command": "cargo test -p core" }));
        assert_eq!(out, "cargo test -p core");
    }

    #[test]
    fn permission_summary_bounds_unknown_tools() {
        let big = "y".repeat(5000);
        let out = summarize_permission_request("mystery", &json!({ "blob": big }));
        assert!(
            out.chars().count() <= 301,
            "unknown tool not bounded: {}",
            out.len()
        );
    }

    #[test]
    fn permission_summary_falls_back_when_no_details() {
        let out = summarize_permission_request("noop", &json!({}));
        assert_eq!(out, "(no details)");
    }

    #[test]
    fn permission_summary_grep_and_glob_scope() {
        let out = summarize_permission_request("grep", &json!({"pattern": "foo", "path": "src"}));
        assert_eq!(out, "pattern: foo\npath: src");
        let glob = summarize_permission_request("glob", &json!({"pattern": "**/*.ts"}));
        assert_eq!(glob, "pattern: **/*.ts");
    }

    #[test]
    fn stop_is_never_gated_by_a_pending_prompt() {
        // 回归：等待权限/提问时点击结束按钮无效。stop 必须始终可用。
        assert_eq!(composer_button_action(true, false, false), ComposerAction::Stop);
        assert_eq!(
            composer_button_action(true, true, false),
            ComposerAction::Stop,
            "stop must stay available while a permission/question prompt is pending"
        );
    }

    #[test]
    fn send_is_blocked_only_while_idle_and_prompted() {
        assert_eq!(
            composer_button_action(false, false, false),
            ComposerAction::Send
        );
        assert_eq!(
            composer_button_action(false, true, false),
            ComposerAction::None,
            "an idle composer awaiting approval must not be able to send"
        );
    }

    #[test]
    fn stopping_disables_button_to_prevent_stop_to_send_flip() {
        // P0-2: Stop RPC in flight — rapid second click must not become Send.
        assert_eq!(
            composer_button_action(false, false, true),
            ComposerAction::None
        );
        assert_eq!(
            composer_button_action(true, false, true),
            ComposerAction::None
        );
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
        use chrono::Utc;
        use code_plus_plus_schema::agent::AgentKind;
        use code_plus_plus_schema::session::Session;
        use code_plus_plus_types::{LocationId, SessionId};

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
        let items = sessions_for_project(
            &sessions,
            &HashSet::new(),
            &std::collections::HashMap::new(),
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, id_new);
        assert_eq!(items[0].title, "New chat");
    }

    #[test]
    fn first_user_message_text_reads_earliest_user_row() {
        use code_plus_plus_schema::message::{Message, MessagePart, MessageRole, PartKind};
        use code_plus_plus_types::{MessageId, SessionId};

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
        use chrono::Utc;
        use code_plus_plus_schema::agent::AgentKind;
        use code_plus_plus_schema::session::Session;
        use code_plus_plus_types::{LocationId, SessionId};

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
        use chrono::Utc;
        use code_plus_plus_schema::agent::AgentKind;
        use code_plus_plus_schema::session::Session;
        use code_plus_plus_types::{LocationId, SessionId};

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
        use code_plus_plus_schema::todo::TodoItem;
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
        use code_plus_plus_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
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
        use code_plus_plus_schema::activity::{
            ActivityKind, ActivityMeta, ActivityNode, ActivityStatus, ActivityTargetKind,
            ActivityTree,
        };
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
        use code_plus_plus_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
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
        use code_plus_plus_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
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
        use code_plus_plus_schema::activity::{
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
                started_at: None,
            })],
        };
        assert!(patch_running_tool_leaf_preview(
            &mut tree,
            "bash-1",
            "[stdout] installing"
        ));
        let ActivityNode::Leaf(leaf) = &tree.roots[0] else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.text.as_deref(), Some("[stdout] installing"));
    }

    #[test]
    fn find_todo_leaf_id_for_index_matches_focus() {
        use code_plus_plus_schema::activity::{
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
                    started_at: None,
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
                    started_at: None,
                }),
            ],
        };
        assert_eq!(
            find_todo_leaf_id_for_index(&tree, 2).as_deref(),
            Some("todo-9")
        );
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
        use code_plus_plus_schema::activity::{
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
                    started_at: None,
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
                    started_at: None,
                }),
            ],
        };
        assert_eq!(find_latest_todo_leaf_id(&tree).as_deref(), Some("todo-9"));
    }

    #[test]
    fn implement_prompt_from_plan_path_matches_spec() {
        let prompt = implement_prompt_from_plan_path(".code-plus-plus/plan/foo/PLAN.md");
        assert!(prompt.contains(".code-plus-plus/plan/foo/PLAN.md"));
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
        use code_plus_plus_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
        let mut meta = ActivityMeta::default();
        meta.first_match_path = Some(".code-plus-plus/plan/x/PLAN.md".into());
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
                path: ".code-plus-plus/plan/x/PLAN.md".into(),
                line: None,
            }
        );
    }

    #[test]
    fn parse_branch_vv_reads_head_and_upstream() {
        let out = "* upgrade/sdk-54 abc1234 [origin/upgrade/sdk-54: ahead 1] continue\n  main def5678 [origin/main] base\n  remotes/origin/upgrade/sdk-54 abc1234 continue\n";
        let branches = parse_branch_vv(out);
        assert_eq!(branches.len(), 3);
        let head = branches.iter().find(|b| b.is_head).unwrap();
        assert_eq!(head.name, "upgrade/sdk-54");
        assert_eq!(head.upstream.as_deref(), Some("origin/upgrade/sdk-54"));
        assert_eq!(head.ahead, 1);
        assert!(!branches.iter().find(|b| b.is_remote).unwrap().is_head);
    }

    #[test]
    fn parse_log_pretty_builds_nodes_and_lanes() {
        let out = "abc1234567890\x1fdef1\x1fxtc\x1f1700000000\x1fcontinue work\x1fHEAD -> upgrade/sdk-54, origin/upgrade/sdk-54\ndef5678abcd\x1fabc1\x1fxtc\x1f1699999999\x1fbase\x1fmaster";
        let nodes = parse_log_pretty(out, Some("abc1234567890"));
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_head);
        assert!(nodes[0].branches.contains(&"upgrade/sdk-54".to_string()));
        assert!(nodes[0]
            .remotes
            .contains(&"origin/upgrade/sdk-54".to_string()));
        assert_eq!(nodes[0].short_sha.len(), 7);
    }

    #[test]
    fn parse_stash_and_remotes() {
        let stash = parse_stash_list("stash@{0}\x1fabc123\x1fWIP\x1f1700000000");
        assert_eq!(stash.len(), 1);
        assert_eq!(stash[0].ref_name, "stash@{0}");
        let remotes = parse_remotes(
            "origin\tgit@github.com:x/y.git (fetch)\norigin\tgit@github.com:x/y.git (push)\n",
        );
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
    }

    #[test]
    fn ahead_behind_and_conflict_helpers() {
        assert_eq!(parse_ahead_behind("1\t2"), (1, 2));
        assert!(is_conflicted_raw("UU file"));
        assert!(!is_conflicted_raw("M  file"));
        assert!(commit_message_error("").is_some());
        assert!(commit_message_error("fix").is_none());
        // Placeholder truncates long branch names (`upgrade/sdk-54` -> `upgrade/sdk-…`).
        assert!(commit_placeholder(Some("upgrade/sdk-54")).contains("upgrade"));
        assert!(commit_placeholder(None).contains("commit"));
        // Cursor-style placeholder always carries the submit hint.
        let ph = commit_placeholder(Some("main"));
        assert!(ph.contains("main") && ph.contains("Ctrl+Enter"));
        assert!(commit_placeholder(Some("")).contains("commit"));
    }

    #[test]
    fn filter_and_hunks() {
        let entries = vec![
            GitStatusEntry {
                path: "src/a.rs".into(),
                staged_label: String::new(),
                unstaged_label: "Modified".into(),
                raw: " M src/a.rs".into(),
            },
            GitStatusEntry {
                path: "src/b.rs".into(),
                staged_label: String::new(),
                unstaged_label: "Modified".into(),
                raw: " M src/b.rs".into(),
            },
        ];
        assert_eq!(filter_changes(&entries, "a.rs").len(), 1);
        assert_eq!(filter_changes(&entries, "").len(), 2);
        let diff =
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n@@ -5 +5 @@\n-c\n+d\n";
        assert_eq!(split_diff_hunks(diff).len(), 2);
        assert_eq!(review_scope_label("master"), "Review diffs vs. master.");
    }

    #[test]
    fn partition_conflicts_separates_merge_group() {
        let staged = vec![GitStatusEntry {
            path: "c.rs".into(),
            staged_label: String::new(),
            unstaged_label: String::new(),
            raw: "UU c.rs".into(),
        }];
        let unstaged = vec![GitStatusEntry {
            path: "a.rs".into(),
            staged_label: String::new(),
            unstaged_label: "Modified".into(),
            raw: " M a.rs".into(),
        }];
        let (cs, cu, cc) = partition_conflicts(staged, unstaged);
        assert!(cs.is_empty());
        assert_eq!(cu.len(), 1);
        assert_eq!(cc.len(), 1);
    }

    #[test]
    fn partition_conflicts_dedups_same_path() {
        let mk = |raw: &str, path: &str| GitStatusEntry {
            path: path.into(),
            staged_label: String::new(),
            unstaged_label: String::new(),
            raw: raw.into(),
        };
        let (cs, cu, cc) =
            partition_conflicts(vec![mk("UU x.rs", "x.rs")], vec![mk("UU x.rs", "x.rs")]);
        assert!(cs.is_empty() && cu.is_empty());
        assert_eq!(cc.len(), 1);
    }

    #[test]
    fn branch_vv_gone_upstream_parses() {
        let out = "* main abc1234 [gone] old subject";
        let branches = parse_branch_vv(out);
        assert_eq!(branches.len(), 1);
        assert!(branches[0].upstream.is_none());
        assert!(branches[0].is_head);
    }

    #[test]
    fn remotes_fetch_only_collapses_push() {
        let remotes = parse_remotes("origin\tgit@github.com:x/y.git (fetch)\n");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].push_url, "git@github.com:x/y.git");
    }

    #[test]
    fn split_hunks_empty_without_headers() {
        assert!(split_diff_hunks("").is_empty());
        assert!(split_diff_hunks("no diff here").is_empty());
    }

    #[test]
    fn git_input_contexts_cover_every_git_input() {
        // Regression lock for dead-backspace reports: every key context set
        // via `set_key_context` in `WorkspaceView::new` for git inputs must
        // be registered in `bind_git_input_keys` (see `GIT_INPUT_CONTEXTS`).
        let contexts = crate::git_panel::GIT_INPUT_CONTEXTS;
        for expected in [
            "GitCommitInput",
            "GitFilterInput",
            "GitBranchInput",
            "GitStashInput",
            "GitReviewScopeInput",
        ] {
            assert!(
                contexts.contains(&expected),
                "git input context {expected} is not bound"
            );
        }
    }

    #[test]
    fn v2_dot_means_unmodified_not_staged() {
        // Converted porcelain-v2 lines (`XY path` with `.` for the clean
        // side) must not leak into the opposite group. Live-test regression:
        // a `.M` worktree-only edit rendered in Staged AND Changes at once.
        let lines = vec![
            ".M work-only.rs".to_string(),
            "M. staged-only.rs".to_string(),
            "MM both.rs".to_string(),
            "A. added.rs".to_string(),
        ];
        let (staged, unstaged, untracked) = classify_git_porcelain(&lines);
        let staged_paths: Vec<_> = staged.iter().map(|e| e.path.as_str()).collect();
        let unstaged_paths: Vec<_> = unstaged.iter().map(|e| e.path.as_str()).collect();
        assert!(!staged_paths.contains(&"work-only.rs"));
        assert!(!unstaged_paths.contains(&"staged-only.rs"));
        assert!(!unstaged_paths.contains(&"added.rs"));
        assert!(staged_paths.contains(&"staged-only.rs"));
        assert!(staged_paths.contains(&"both.rs") && unstaged_paths.contains(&"both.rs"));
        assert!(staged_paths.contains(&"added.rs"));
        assert!(untracked.is_empty());
    }

    #[test]
    fn filter_is_case_insensitive() {
        let entries = vec![GitStatusEntry {
            path: "SRC/App.TS".into(),
            staged_label: String::new(),
            unstaged_label: "Modified".into(),
            raw: " M SRC/App.TS".into(),
        }];
        assert_eq!(filter_changes(&entries, "app.ts").len(), 1);
        assert_eq!(filter_changes(&entries, "zzz").len(), 0);
    }
}
