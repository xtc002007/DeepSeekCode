//! Projects session events + messages into Cursor-style Activity trees.

use chrono::{DateTime, Utc};
use deepseekcode_schema::activity::{
    ActivityGroup, ActivityGroupKind, ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode,
    ActivityStatus, ActivitySummary, ActivityTargetKind, ActivityTree, DiffStats,
};
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::message::{Message, MessageRole, PartKind};
use deepseekcode_schema::tool::ToolResultEnvelope;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ChildSessionProjection {
    pub events: Vec<EventEnvelope>,
    pub messages: Vec<Message>,
}

pub struct ActivityProjector;

struct TurnBuilder {
    id: String,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    children: Vec<ActivityNode>,
    pending_tools: std::collections::HashMap<String, PendingTool>,
    reasoning_text: String,
    reasoning_started: Option<DateTime<Utc>>,
    assistant_text: String,
    assistant_message_id: Option<String>,
}

#[derive(Clone)]
struct PendingTool {
    name: String,
    input: Value,
    title: Option<String>,
    metadata: Value,
    started_at: DateTime<Utc>,
}

impl ActivityProjector {
    /// Incrementally apply one event; returns whether the tree was rebuilt or patched.
    pub fn apply_event(
        state: &mut super::incremental::ActivityProjectionState,
        event: &EventEnvelope,
    ) -> super::incremental::ActivityApplyOutcome {
        state.apply_event(event)
    }

    pub fn project(events: &[EventEnvelope], messages: &[Message]) -> ActivityTree {
        Self::project_with_children(events, messages, &HashMap::new())
    }

    pub fn project_with_children(
        events: &[EventEnvelope],
        messages: &[Message],
        children: &HashMap<String, ChildSessionProjection>,
    ) -> ActivityTree {
        let user_messages: HashMap<String, Message> = messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .map(|m| (m.id.to_string(), m.clone()))
            .collect();

        let mut roots: Vec<ActivityNode> = Vec::new();
        let mut turn: Option<TurnBuilder> = None;

        for event in events {
            match event.kind {
                EventKind::ProviderTurnStarted => {
                    if let Some(mut finished) = turn.take() {
                        roots.push(ActivityNode::Group(finished.finish()));
                    }
                    turn = Some(TurnBuilder {
                        id: format!("turn-{}", event.seq),
                        started_at: Some(event.created_at),
                        ended_at: None,
                        children: Vec::new(),
                        pending_tools: std::collections::HashMap::new(),
                        reasoning_text: String::new(),
                        reasoning_started: None,
                        assistant_text: String::new(),
                        assistant_message_id: None,
                    });
                }
                EventKind::MessageDelta => {
                    if let Some(t) = turn.as_mut() {
                        if event.payload.get("part_kind").and_then(|v| v.as_str())
                            == Some("reasoning")
                        {
                            if t.reasoning_started.is_none() {
                                t.reasoning_started = Some(event.created_at);
                            }
                            if let Some(delta) = event.payload.get("delta").and_then(|v| v.as_str())
                            {
                                t.reasoning_text.push_str(delta);
                            }
                        } else if let Some(delta) =
                            event.payload.get("delta").and_then(|v| v.as_str())
                        {
                            if !delta.is_empty() {
                                if t.assistant_message_id.is_none() {
                                    t.assistant_message_id = event
                                        .payload
                                        .get("message_id")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                }
                                t.assistant_text.push_str(delta);
                                let id = t
                                    .assistant_message_id
                                    .clone()
                                    .unwrap_or_else(|| format!("assistant-{}", event.seq));
                                replace_or_push_assistant(
                                    &mut t.children,
                                    assistant_leaf(&id, &t.assistant_text, ActivityStatus::Running),
                                );
                            }
                        }
                    }
                }
                EventKind::ToolStarted => {
                    let id = event.payload["tool_call_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let pending = PendingTool {
                        name: event.payload["tool_name"]
                            .as_str()
                            .unwrap_or("tool")
                            .to_string(),
                        input: event.payload.get("input").cloned().unwrap_or(Value::Null),
                        title: event
                            .payload
                            .get("title")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        metadata: event
                            .payload
                            .get("metadata")
                            .cloned()
                            .unwrap_or(Value::Null),
                        started_at: event.created_at,
                    };
                    if let Some(t) = turn.as_mut() {
                        t.pending_tools.insert(id.clone(), pending.clone());
                        t.children
                            .push(ActivityNode::Leaf(running_leaf(&id, &pending)));
                    } else {
                        roots.push(ActivityNode::Leaf(running_leaf(&id, &pending)));
                    }
                }
                EventKind::ToolCompleted => {
                    let id = event.payload["tool_call_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let pending = turn.as_mut().and_then(|t| t.pending_tools.remove(&id));
                    let name = event.payload["tool_name"]
                        .as_str()
                        .or(pending.as_ref().map(|p| p.name.as_str()))
                        .unwrap_or("tool");
                    if name == "task" {
                        let leaf = completed_leaf_from_event(event, pending);
                        let node = task_subagent_node(&leaf, event, children);
                        if let Some(t) = turn.as_mut() {
                            replace_or_push_node(t, &id, node);
                        } else {
                            roots.push(node);
                        }
                        continue;
                    }
                    if name == "question" {
                        let leaf = completed_leaf_from_event(event, pending);
                        let answered = ActivityLeaf {
                            verb: "Answered".into(),
                            status: ActivityStatus::Completed,
                            ..leaf
                        };
                        if let Some(t) = turn.as_mut() {
                            replace_or_push_leaf(t, &id, answered);
                        } else {
                            roots.push(ActivityNode::Leaf(answered));
                        }
                        continue;
                    }
                    if name == "plan_complete" || name == "plan.complete" {
                        let leaf = {
                            let base = completed_leaf_from_event(event, pending.clone());
                            if base.status == ActivityStatus::Failed {
                                base
                            } else {
                                plan_ready_leaf_from_event(event, pending)
                            }
                        };
                        if let Some(t) = turn.as_mut() {
                            replace_or_push_leaf(t, &id, leaf);
                        } else {
                            roots.push(ActivityNode::Leaf(leaf));
                        }
                        continue;
                    }
                    let leaf = completed_leaf_from_event(event, pending);
                    if let Some(t) = turn.as_mut() {
                        replace_or_push_leaf(t, &id, leaf);
                    } else {
                        roots.push(ActivityNode::Leaf(leaf));
                    }
                }
                EventKind::ToolProgress => {
                    let id = event.payload["tool_call_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let chunk = event
                        .payload
                        .get("chunk")
                        .or_else(|| event.payload.get("line"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if chunk.is_empty() {
                        continue;
                    }
                    let stream = event
                        .payload
                        .get("stream")
                        .and_then(|v| v.as_str())
                        .unwrap_or("stdout");
                    let preview = format!("[{stream}] {chunk}");
                    if let Some(t) = turn.as_mut() {
                        update_running_tool_preview(t, &id, &preview);
                    }
                }
                EventKind::TodoUpdated => {
                    let leaf = todo_leaf_from_event(event);
                    if let Some(t) = turn.as_mut() {
                        t.children.push(ActivityNode::Leaf(leaf));
                    } else {
                        roots.push(ActivityNode::Leaf(leaf));
                    }
                }
                EventKind::PermissionRequested
                | EventKind::PermissionResolved
                | EventKind::PolicyPrompted
                | EventKind::PolicyResolved => {
                    let leaf = permission_leaf(event);
                    if let Some(t) = turn.as_mut() {
                        t.children.push(ActivityNode::Leaf(leaf));
                    } else {
                        roots.push(ActivityNode::Leaf(leaf));
                    }
                }
                EventKind::QuestionRequested => {
                    let leaf = ActivityLeaf {
                        id: format!("question-{}", event.seq),
                        kind: ActivityKind::Question,
                        verb: "Asked".into(),
                        target: ActivityTargetKind::Generic {
                            label: event.payload["question"].as_str().unwrap_or("").to_string(),
                        },
                        meta: ActivityMeta::default(),
                        status: ActivityStatus::Running,
                        detail_ref: None,
                        text: None,
                    };
                    if let Some(t) = turn.as_mut() {
                        t.children.push(ActivityNode::Leaf(leaf));
                    } else {
                        roots.push(ActivityNode::Leaf(leaf));
                    }
                }
                EventKind::SessionIdle
                | EventKind::SessionInterrupted
                | EventKind::SessionCompacted => {
                    if let Some(mut t) = turn.take() {
                        t.ended_at = Some(event.created_at);
                        roots.push(ActivityNode::Group(t.finish()));
                    }
                    roots.push(ActivityNode::Group(session_meta_group(event)));
                }
                EventKind::PromptAdmitted | EventKind::Prompted => {
                    let id = event
                        .payload
                        .get("message_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| format!("user-{}", event.seq));
                    let text = event
                        .payload
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| {
                            user_messages
                                .get(&id)
                                .map(|m| join_text(m))
                                .filter(|t| !t.is_empty())
                        });
                    if let Some(text) = text {
                        if !text.is_empty()
                            && !roots.iter().any(|n| user_message_matches(n, &id, &text))
                        {
                            roots.push(ActivityNode::Leaf(user_leaf(&id, &text)));
                        }
                    }
                }
                EventKind::MessageCompleted => {
                    if let Some(t) = turn.as_mut() {
                        if !t.reasoning_text.is_empty() {
                            let duration_ms = t
                                .reasoning_started
                                .map(|s| (event.created_at - s).num_milliseconds().max(0) as u64)
                                .unwrap_or(0);
                            t.children.push(ActivityNode::Group(ActivityGroup {
                                id: format!("thought-{}", event.seq),
                                kind: ActivityGroupKind::Thought,
                                label: format_thought_label(duration_ms),
                                summary: None,
                                children: vec![ActivityNode::Leaf(ActivityLeaf {
                                    id: format!("reasoning-{}", event.seq),
                                    kind: ActivityKind::Reasoning,
                                    verb: "Thought".into(),
                                    target: ActivityTargetKind::None,
                                    meta: ActivityMeta {
                                        duration_ms: Some(duration_ms),
                                        ..Default::default()
                                    },
                                    status: ActivityStatus::Completed,
                                    detail_ref: Some(format!("reasoning-{}", event.seq)),
                                    text: Some(t.reasoning_text.clone()),
                                })],
                                collapsed_default: true,
                            }));
                            t.reasoning_text.clear();
                            t.reasoning_started = None;
                        }
                    }
                    let message_id = event
                        .payload
                        .get("message_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let event_content = event
                        .payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let buffered = turn
                        .as_ref()
                        .map(|t| t.assistant_text.clone())
                        .unwrap_or_default();
                    let content = if event_content.len() >= buffered.len() {
                        event_content.to_string()
                    } else {
                        buffered
                    };
                    if let Some(t) = turn.as_mut() {
                        t.assistant_text.clear();
                        t.assistant_message_id = None;
                    }
                    if !content.is_empty() {
                        let leaf_id = if message_id.is_empty() {
                            format!("assistant-{}", event.seq)
                        } else {
                            message_id
                        };
                        let leaf = assistant_leaf(&leaf_id, &content, ActivityStatus::Completed);
                        if let Some(t) = turn.as_mut() {
                            replace_or_push_assistant(&mut t.children, leaf);
                        } else {
                            roots.push(ActivityNode::Leaf(leaf));
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(mut t) = turn {
            roots.push(ActivityNode::Group(t.finish()));
        }

        merge_missing_user_messages_by_seq(&mut roots, messages);

        for msg in messages {
            if msg.role != MessageRole::Assistant {
                continue;
            }
            let text = join_text(msg);
            if text.is_empty() {
                continue;
            }
            let id = msg.id.to_string();
            if upgrade_assistant_in_tree(&mut roots, &id, &text) {
                continue;
            }
            append_assistant_message(&mut roots, &id, &text);
        }

        ActivityTree { roots }
    }
}

impl TurnBuilder {
    fn finish(mut self) -> ActivityGroup {
        if !self.reasoning_text.is_empty() {
            self.children.push(ActivityNode::Leaf(ActivityLeaf {
                id: format!("{}-reasoning", self.id),
                kind: ActivityKind::Reasoning,
                verb: "Thought".into(),
                target: ActivityTargetKind::None,
                meta: ActivityMeta::default(),
                status: ActivityStatus::Completed,
                detail_ref: None,
                text: Some(self.reasoning_text),
            }));
        }
        if !self.assistant_text.is_empty() {
            let id = self
                .assistant_message_id
                .clone()
                .unwrap_or_else(|| format!("{}-assistant", self.id));
            replace_or_push_assistant(
                &mut self.children,
                assistant_leaf(&id, &self.assistant_text, ActivityStatus::Completed),
            );
        }
        let duration_ms = match (self.started_at, self.ended_at) {
            (Some(s), Some(e)) => (e - s).num_milliseconds().max(0) as u64,
            (Some(s), None) => (Utc::now() - s).num_milliseconds().max(0) as u64,
            _ => 0,
        };
        let summary = summarize_children(&self.children);
        ActivityGroup {
            id: self.id,
            kind: ActivityGroupKind::ProviderTurn,
            label: format_worked_label(duration_ms),
            summary: Some(summary),
            children: self.children,
            collapsed_default: false,
        }
    }
}

fn summarize_children(children: &[ActivityNode]) -> ActivitySummary {
    let mut summary = ActivitySummary::default();
    for node in children {
        let ActivityNode::Leaf(leaf) = node else {
            continue;
        };
        match leaf.kind {
            ActivityKind::Write | ActivityKind::Edit | ActivityKind::ApplyPatch => {
                summary.files_edited += 1;
                if let Some(stats) = &leaf.meta.diff_stats {
                    summary.lines_added += stats.added;
                    summary.lines_removed += stats.removed;
                }
            }
            ActivityKind::Read | ActivityKind::Glob | ActivityKind::List => {
                summary.files_explored += 1;
            }
            ActivityKind::Grep | ActivityKind::WebSearch => summary.searches += 1,
            ActivityKind::Bash => summary.commands += 1,
            _ => {}
        }
    }
    summary
}

fn format_worked_label(duration_ms: u64) -> String {
    let secs = duration_ms / 1000;
    if secs == 0 {
        "Worked for <1s".into()
    } else {
        format!("Worked for {secs}s")
    }
}

fn format_thought_label(duration_ms: u64) -> String {
    let secs = duration_ms / 1000;
    if secs == 0 {
        "Thought for <1s".into()
    } else {
        format!("Thought for {secs}s")
    }
}

fn join_text(msg: &Message) -> String {
    msg.parts
        .iter()
        .filter_map(|p| match p.kind {
            PartKind::Text => p.text.clone(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn user_message_matches(node: &ActivityNode, id: &str, text: &str) -> bool {
    match node {
        ActivityNode::Leaf(l) if l.kind == ActivityKind::UserMessage => {
            l.id == id || l.text.as_deref() == Some(text)
        }
        _ => false,
    }
}

fn root_sort_seq(node: &ActivityNode) -> i64 {
    match node {
        ActivityNode::Group(g) => {
            if let Some(seq) = g.id.strip_prefix("turn-").and_then(|s| s.parse().ok()) {
                return seq;
            }
            if let Some(seq) = g.id.strip_prefix("meta-").and_then(|s| s.parse().ok()) {
                return seq;
            }
            i64::MAX
        }
        ActivityNode::Leaf(_) => i64::MAX,
    }
}

fn merge_missing_user_messages_by_seq(roots: &mut Vec<ActivityNode>, messages: &[Message]) {
    let mut pending: Vec<(i64, ActivityNode)> = Vec::new();
    for msg in messages {
        if msg.role != MessageRole::User {
            continue;
        }
        let text = join_text(msg);
        if text.is_empty() {
            continue;
        }
        let id = msg.id.to_string();
        if roots.iter().any(|n| user_message_matches(n, &id, &text)) {
            continue;
        }
        pending.push((msg.seq, ActivityNode::Leaf(user_leaf(&id, &text))));
    }
    pending.sort_by_key(|(seq, _)| *seq);
    for (msg_seq, node) in pending {
        let idx = roots
            .iter()
            .position(|n| root_sort_seq(n) > msg_seq)
            .unwrap_or(roots.len());
        roots.insert(idx, node);
    }
}

fn user_leaf(id: &str, text: &str) -> ActivityLeaf {
    ActivityLeaf {
        id: id.to_string(),
        kind: ActivityKind::UserMessage,
        verb: "You".into(),
        target: ActivityTargetKind::None,
        meta: ActivityMeta::default(),
        status: ActivityStatus::Completed,
        detail_ref: None,
        text: Some(text.to_string()),
    }
}

fn assistant_leaf(id: &str, text: &str, status: ActivityStatus) -> ActivityLeaf {
    ActivityLeaf {
        id: id.to_string(),
        kind: ActivityKind::AssistantMessage,
        verb: "Assistant".into(),
        target: ActivityTargetKind::None,
        meta: ActivityMeta::default(),
        status,
        detail_ref: None,
        text: Some(text.to_string()),
    }
}

fn replace_or_push_assistant(children: &mut Vec<ActivityNode>, leaf: ActivityLeaf) {
    let id = leaf.id.clone();
    for node in children.iter_mut() {
        if let ActivityNode::Leaf(existing) = node {
            if existing.kind == ActivityKind::AssistantMessage && existing.id == id {
                existing.text = leaf.text;
                existing.status = leaf.status;
                return;
            }
        }
    }
    children.push(ActivityNode::Leaf(leaf));
}

fn upgrade_assistant_in_tree(nodes: &mut [ActivityNode], id: &str, text: &str) -> bool {
    for node in nodes {
        match node {
            ActivityNode::Leaf(leaf)
                if leaf.kind == ActivityKind::AssistantMessage && leaf.id == id =>
            {
                let existing_len = leaf
                    .text
                    .as_deref()
                    .map(str::chars)
                    .map(|c| c.count())
                    .unwrap_or(0);
                let new_len = text.chars().count();
                if new_len > existing_len {
                    leaf.text = Some(text.to_string());
                    leaf.status = ActivityStatus::Completed;
                }
                return true;
            }
            ActivityNode::Group(group) => {
                if upgrade_assistant_in_tree(&mut group.children, id, text) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn append_assistant_message(nodes: &mut Vec<ActivityNode>, id: &str, text: &str) {
    for node in nodes.iter_mut().rev() {
        if let ActivityNode::Group(group) = node {
            if group.kind == ActivityGroupKind::ProviderTurn {
                group.children.push(ActivityNode::Leaf(assistant_leaf(
                    id,
                    text,
                    ActivityStatus::Completed,
                )));
                return;
            }
        }
    }
    nodes.push(ActivityNode::Leaf(assistant_leaf(
        id,
        text,
        ActivityStatus::Completed,
    )));
}

fn session_meta_group(event: &EventEnvelope) -> ActivityGroup {
    let label = match event.kind {
        EventKind::SessionIdle => "Session idle",
        EventKind::SessionInterrupted => "Session interrupted",
        EventKind::SessionCompacted => "Context compacted",
        _ => "Session",
    };
    ActivityGroup {
        id: format!("meta-{}", event.seq),
        kind: ActivityGroupKind::SessionMeta,
        label: label.into(),
        summary: None,
        children: vec![ActivityNode::Leaf(ActivityLeaf {
            id: format!("meta-leaf-{}", event.seq),
            kind: ActivityKind::SessionMeta,
            verb: label.into(),
            target: ActivityTargetKind::None,
            meta: ActivityMeta::default(),
            status: ActivityStatus::Completed,
            detail_ref: None,
            text: None,
        })],
        collapsed_default: true,
    }
}

fn permission_leaf(event: &EventEnvelope) -> ActivityLeaf {
    let tool = event.payload["tool_name"]
        .as_str()
        .unwrap_or("tool")
        .to_string();
    let scope = event.payload["scope"].as_str().unwrap_or(&tool).to_string();
    let policy_id = event.payload["policy_id"]
        .as_str()
        .or_else(|| event.payload["permission_id"].as_str())
        .map(String::from);
    let verb = match event.kind {
        EventKind::PolicyPrompted | EventKind::PermissionRequested => {
            format!("Policy · {scope} · {tool} · Required")
        }
        EventKind::PolicyResolved => {
            let reply = event.payload["reply"]
                .as_str()
                .or_else(|| event.payload["status"].as_str())
                .unwrap_or("");
            match reply.to_ascii_lowercase().as_str() {
                "once" | "permit" | "permitted" | "allowed" => {
                    format!("Policy · {scope} · {tool} · Allowed once")
                }
                "project" | "always" => format!("Policy · {scope} · {tool} · Allowed (project)"),
                "auto" => format!("Policy · {scope} · {tool} · Allowed (auto)"),
                "reject" | "rejected" | "denied" | "deny" => {
                    format!("Policy · {scope} · {tool} · Denied")
                }
                other => format!("Policy · {scope} · {tool} · {other}"),
            }
        }
        EventKind::PermissionResolved => {
            let status = event.payload["status"]
                .as_str()
                .or_else(|| event.payload["decision"].as_str())
                .unwrap_or("");
            if status.eq_ignore_ascii_case("deny") || status.eq_ignore_ascii_case("denied") {
                format!("Policy · {scope} · {tool} · Denied")
            } else {
                format!("Policy · {scope} · {tool} · Allowed")
            }
        }
        _ => format!("Policy · {scope} · {tool}"),
    };
    ActivityLeaf {
        id: format!("perm-{}", event.seq),
        kind: ActivityKind::Permission,
        verb,
        target: ActivityTargetKind::Generic { label: tool },
        meta: ActivityMeta::default(),
        status: ActivityStatus::Completed,
        detail_ref: policy_id,
        text: None,
    }
}

fn todo_leaf_from_event(event: &EventEnvelope) -> ActivityLeaf {
    let todos = event.payload.get("todos").and_then(|v| v.as_array());
    let pending = todos
        .map(|t| {
            t.iter()
                .filter(|i| i.get("status").and_then(|s| s.as_str()) == Some("pending"))
                .count() as u32
        })
        .unwrap_or(0);
    let in_progress = todos
        .map(|t| {
            t.iter()
                .filter(|i| i.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
                .count() as u32
        })
        .unwrap_or(0);
    let focus_index = todos.and_then(|items| {
        items
            .iter()
            .position(|i| i.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
            .or_else(|| {
                items
                    .iter()
                    .position(|i| i.get("status").and_then(|s| s.as_str()) == Some("pending"))
            })
            .map(|i| i as u32)
    });
    ActivityLeaf {
        id: format!("todo-{}", event.seq),
        kind: ActivityKind::TodoWrite,
        verb: "Updated todos".into(),
        target: ActivityTargetKind::TodoList,
        meta: ActivityMeta {
            todo_pending: Some(pending),
            todo_in_progress: Some(in_progress),
            todo_focus_index: focus_index,
            ..Default::default()
        },
        status: ActivityStatus::Completed,
        detail_ref: Some(format!("todo-{}", event.seq)),
        text: None,
    }
}

fn replace_or_push_leaf(turn: &mut TurnBuilder, id: &str, leaf: ActivityLeaf) {
    if let Some(pos) = turn
        .children
        .iter()
        .position(|n| matches!(n, ActivityNode::Leaf(l) if l.id == id))
    {
        turn.children[pos] = ActivityNode::Leaf(leaf);
    } else {
        turn.children.push(ActivityNode::Leaf(leaf));
    }
}

fn replace_or_push_node(turn: &mut TurnBuilder, id: &str, node: ActivityNode) {
    if let Some(pos) = turn
        .children
        .iter()
        .position(|n| matches!(n, ActivityNode::Leaf(l) if l.id == id))
    {
        turn.children[pos] = node;
    } else {
        turn.children.push(node);
    }
}

fn update_running_tool_preview(turn: &mut TurnBuilder, id: &str, preview: &str) {
    if let Some(pos) = turn.children.iter().position(
        |n| matches!(n, ActivityNode::Leaf(l) if l.id == id && l.status == ActivityStatus::Running),
    ) {
        if let ActivityNode::Leaf(leaf) = &mut turn.children[pos] {
            leaf.text = Some(preview.to_string());
        }
    }
}

fn task_subagent_node(
    leaf: &ActivityLeaf,
    event: &EventEnvelope,
    children: &HashMap<String, ChildSessionProjection>,
) -> ActivityNode {
    let output = event.payload.get("output").cloned().unwrap_or(Value::Null);
    let envelope = ToolResultEnvelope::from_stored(&output);
    let session_id = envelope
        .output
        .get("session_id")
        .or_else(|| envelope.metadata.get("session_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let agent = envelope
        .output
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();
    let mut group_children = vec![ActivityNode::Leaf(ActivityLeaf {
        target: ActivityTargetKind::Session {
            session_id: session_id.clone(),
            label: Some(agent.clone()),
        },
        ..leaf.clone()
    })];
    if let Some(child) = children.get(&session_id) {
        let nested = ActivityProjector::project(&child.events, &child.messages);
        group_children.extend(nested.roots);
    }
    ActivityNode::Group(ActivityGroup {
        id: format!("task-group-{}", leaf.id),
        kind: ActivityGroupKind::TaskSubagent,
        label: format!("Task {agent}"),
        summary: None,
        children: group_children,
        collapsed_default: true,
    })
}

fn running_leaf(id: &str, pending: &PendingTool) -> ActivityLeaf {
    let (kind, verb, target, meta) = map_tool(&pending.name, &pending.input, &pending.metadata);
    ActivityLeaf {
        id: id.to_string(),
        kind,
        verb,
        target,
        meta,
        status: ActivityStatus::Running,
        detail_ref: Some(id.to_string()),
        text: pending.title.clone(),
    }
}

fn completed_leaf_from_event(event: &EventEnvelope, pending: Option<PendingTool>) -> ActivityLeaf {
    let id = event.payload["tool_call_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let name = event.payload["tool_name"]
        .as_str()
        .or(pending.as_ref().map(|p| p.name.as_str()))
        .unwrap_or("tool");
    let input = pending
        .as_ref()
        .map(|p| p.input.clone())
        .unwrap_or(Value::Null);
    let metadata = event
        .payload
        .get("metadata")
        .cloned()
        .or_else(|| pending.as_ref().map(|p| p.metadata.clone()))
        .unwrap_or(Value::Null);
    let output = event.payload.get("output").cloned().unwrap_or(Value::Null);
    let envelope = ToolResultEnvelope::from_stored(&output);
    let failed = envelope.output.get("error").is_some()
        || envelope.output.get("success") == Some(&Value::Bool(false));
    let (kind, verb, target, mut meta) = map_tool(name, &input, &metadata);
    if let Some(ms) = event.payload.get("duration_ms").and_then(|v| v.as_u64()) {
        meta.duration_ms = Some(ms);
    }
    if meta.diff_stats.is_none() {
        meta.diff_stats =
            parse_diff_stats(&metadata).or_else(|| parse_diff_stats(&envelope.metadata));
    }
    ActivityLeaf {
        id: id.clone(),
        kind,
        verb,
        target,
        meta,
        status: if failed {
            ActivityStatus::Failed
        } else {
            ActivityStatus::Completed
        },
        detail_ref: Some(id),
        text: event
            .payload
            .get("title")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

fn plan_ready_leaf_from_event(event: &EventEnvelope, pending: Option<PendingTool>) -> ActivityLeaf {
    let mut leaf = completed_leaf_from_event(event, pending);
    let output = event.payload.get("output").cloned().unwrap_or(Value::Null);
    let envelope = ToolResultEnvelope::from_stored(&output);
    let plan_path = envelope
        .output
        .get("plan_path")
        .and_then(|v| v.as_str())
        .map(String::from);
    let title = envelope
        .output
        .get("title")
        .and_then(|v| v.as_str())
        .map(String::from);
    leaf.kind = ActivityKind::PlanReady;
    leaf.verb = "Plan ready".into();
    if let Some(path) = plan_path {
        leaf.meta.first_match_path = Some(path);
    }
    leaf.meta.title = title.clone();
    if let Some(t) = title {
        leaf.target = ActivityTargetKind::Generic { label: t };
    }
    leaf
}

fn parse_diff_stats(value: &Value) -> Option<DiffStats> {
    value.get("diff_stats").and_then(|s| {
        Some(DiffStats {
            added: s.get("added")?.as_u64()? as u32,
            removed: s.get("removed")?.as_u64()? as u32,
        })
    })
}

fn map_tool(
    name: &str,
    input: &Value,
    metadata: &Value,
) -> (ActivityKind, String, ActivityTargetKind, ActivityMeta) {
    let mut meta = ActivityMeta::default();
    meta.diff_stats = parse_diff_stats(metadata);
    if let Some(c) = metadata.get("match_count").and_then(|v| v.as_u64()) {
        meta.match_count = Some(c as u32);
    }
    if let Some(c) = metadata.get("result_count").and_then(|v| v.as_u64()) {
        meta.match_count = Some(c as u32);
    }
    if let Some(c) = metadata.get("file_count").and_then(|v| v.as_u64()) {
        meta.file_count = Some(c as u32);
    }
    if let Some(c) = metadata.get("entry_count").and_then(|v| v.as_u64()) {
        meta.entry_count = Some(c as u32);
    }
    if let Some(first) = metadata.get("first_match") {
        if let Some(path) = first.get("path").and_then(|v| v.as_str()) {
            meta.first_match_path = Some(path.to_string());
        }
        if let Some(line) = first.get("line").and_then(|v| v.as_u64()) {
            meta.first_match_line = Some(line as u32);
        }
    }
    if let Some(c) = metadata.get("exit_code").and_then(|v| v.as_i64()) {
        meta.exit_code = Some(c);
    }
    if let Some(p) = metadata.get("first_match_path").and_then(|v| v.as_str()) {
        meta.first_match_path = Some(p.to_string());
    }
    if let Some(l) = metadata.get("first_match_line").and_then(|v| v.as_u64()) {
        meta.first_match_line = Some(l as u32);
    }
    if meta.first_match_path.is_none() {
        if let Some(p) = metadata
            .get("path")
            .or_else(|| metadata.get("target"))
            .or_else(|| metadata.get("base_dir"))
            .and_then(|v| v.as_str())
        {
            meta.first_match_path = Some(p.to_string());
        }
    }

    match name {
        "read" => {
            let path = metadata
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| input.get("path").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let line_start = metadata
                .get("line_start")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let line_end = metadata
                .get("line_end")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            (
                ActivityKind::Read,
                "Read".into(),
                ActivityTargetKind::File {
                    path,
                    line_start,
                    line_end,
                },
                meta,
            )
        }
        "write" => {
            let path = file_path(input, metadata);
            (
                ActivityKind::Write,
                "Wrote".into(),
                ActivityTargetKind::File {
                    path,
                    line_start: None,
                    line_end: None,
                },
                meta,
            )
        }
        "edit" => {
            let path = file_path(input, metadata);
            (
                ActivityKind::Edit,
                "Edited".into(),
                ActivityTargetKind::File {
                    path,
                    line_start: None,
                    line_end: None,
                },
                meta,
            )
        }
        "apply_patch" => (
            ActivityKind::ApplyPatch,
            "Patched".into(),
            ActivityTargetKind::Generic {
                label: metadata
                    .get("files")
                    .and_then(|f| f.as_array())
                    .map(|a| format!("{} files", a.len()))
                    .unwrap_or_else(|| "patch".into()),
            },
            meta,
        ),
        "grep" => (
            ActivityKind::Grep,
            "Searched".into(),
            ActivityTargetKind::Pattern {
                pattern: input
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                scope: input.get("path").and_then(|v| v.as_str()).map(String::from),
            },
            meta,
        ),
        "glob" => (
            ActivityKind::Glob,
            "Explored".into(),
            ActivityTargetKind::Pattern {
                pattern: input
                    .get("pattern")
                    .or_else(|| input.get("glob_pattern"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .into(),
                scope: None,
            },
            meta,
        ),
        "list" => (
            ActivityKind::List,
            "Listed".into(),
            ActivityTargetKind::Pattern {
                pattern: input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".")
                    .into(),
                scope: None,
            },
            meta,
        ),
        "bash" => (
            ActivityKind::Bash,
            "Ran".into(),
            ActivityTargetKind::Command {
                command: input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            },
            meta,
        ),
        "webfetch" => (
            ActivityKind::WebFetch,
            "Fetched".into(),
            ActivityTargetKind::Url {
                url: input
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            },
            meta,
        ),
        "websearch" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let url = metadata
                .get("first_url")
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(url) = url {
                (
                    ActivityKind::WebSearch,
                    "Searched web".into(),
                    ActivityTargetKind::Url { url },
                    meta,
                )
            } else {
                (
                    ActivityKind::WebSearch,
                    "Searched web".into(),
                    ActivityTargetKind::Generic {
                        label: query.into(),
                    },
                    meta,
                )
            }
        }
        "skill" => (
            ActivityKind::Skill,
            "Loaded skill".into(),
            ActivityTargetKind::Skill {
                name: input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            },
            meta,
        ),
        "todowrite" | "todo_write" => (
            ActivityKind::TodoWrite,
            "Updated todos".into(),
            ActivityTargetKind::TodoList,
            meta,
        ),
        "todoread" => (
            ActivityKind::TodoRead,
            "Read todos".into(),
            ActivityTargetKind::TodoList,
            meta,
        ),
        "task" => (
            ActivityKind::Task,
            "Task".into(),
            ActivityTargetKind::Session {
                session_id: metadata
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                label: input
                    .get("agent")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            },
            meta,
        ),
        "question" => (
            ActivityKind::Question,
            "Asked".into(),
            ActivityTargetKind::Generic {
                label: input
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            },
            meta,
        ),
        "plan_complete" | "plan.complete" => (
            ActivityKind::PlanReady,
            "Plan ready".into(),
            ActivityTargetKind::Generic {
                label: input
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Plan")
                    .into(),
            },
            meta,
        ),
        "lsp" => (
            ActivityKind::Lsp,
            "LSP".into(),
            ActivityTargetKind::Generic {
                label: input
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            },
            meta,
        ),
        other if other.contains('_') => (
            ActivityKind::Mcp,
            other.into(),
            ActivityTargetKind::Generic {
                label: metadata
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(other)
                    .into(),
            },
            meta,
        ),
        other => (
            ActivityKind::Mcp,
            other.into(),
            ActivityTargetKind::Generic {
                label: other.into(),
            },
            meta,
        ),
    }
}

fn file_path(input: &Value, metadata: &Value) -> String {
    metadata
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("path").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use deepseekcode_schema::event::EventKind;

    fn ev(kind: EventKind, payload: Value) -> EventEnvelope {
        static SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        EventEnvelope {
            seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            session_id: None,
            payload,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn merges_tool_started_and_completed() {
        let events = vec![
            ev(EventKind::ProviderTurnStarted, serde_json::json!({})),
            ev(
                EventKind::ToolStarted,
                serde_json::json!({
                    "tool_call_id": "c1",
                    "tool_name": "read",
                    "input": { "path": "lib.rs" },
                    "metadata": { "path": "lib.rs", "line_start": 1, "line_end": 17 }
                }),
            ),
            ev(
                EventKind::ToolCompleted,
                serde_json::json!({
                    "tool_call_id": "c1",
                    "tool_name": "read",
                    "title": "Read lib.rs",
                    "metadata": { "path": "lib.rs", "line_start": 1, "line_end": 17 },
                    "duration_ms": 12,
                    "output": { "title": "Read lib.rs", "output": { "content": "fn main(){}" }, "metadata": {} }
                }),
            ),
        ];
        let tree = ActivityProjector::project(&events, &[]);
        let groups: Vec<_> = tree
            .roots
            .iter()
            .filter(|n| matches!(n, ActivityNode::Group(g) if g.kind == ActivityGroupKind::ProviderTurn))
            .collect();
        assert_eq!(groups.len(), 1);
        let ActivityNode::Group(group) = groups[0] else {
            panic!("expected group");
        };
        assert!(group.label.starts_with("Worked for"));
        assert_eq!(group.children.len(), 1);
        let ActivityNode::Leaf(leaf) = &group.children[0] else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.kind, ActivityKind::Read);
        assert_eq!(leaf.verb, "Read");
    }

    #[test]
    fn policy_prompt_and_resolved_leaves() {
        let events = vec![
            ev(
                EventKind::PolicyPrompted,
                serde_json::json!({
                    "policy_id": "p1",
                    "tool_name": "bash",
                    "scope": "bash",
                }),
            ),
            ev(
                EventKind::PolicyResolved,
                serde_json::json!({
                    "policy_id": "p1",
                    "tool_name": "bash",
                    "scope": "bash",
                    "reply": "project",
                }),
            ),
        ];
        let tree = ActivityProjector::project(&events, &[]);
        let leaves: Vec<_> = tree
            .roots
            .iter()
            .filter_map(|n| match n {
                ActivityNode::Leaf(l) => Some(l),
                _ => None,
            })
            .collect();
        assert_eq!(leaves.len(), 2);
        assert!(leaves[0].verb.contains("Required"));
        assert!(leaves[1].verb.contains("Allowed (project)"));
    }
}
