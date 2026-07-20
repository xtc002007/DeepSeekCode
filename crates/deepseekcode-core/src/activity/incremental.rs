//! Incremental activity-tree updates for streaming SSE events.

use deepseekcode_schema::activity::{
    ActivityKind, ActivityLeaf, ActivityMeta, ActivityNode, ActivityStatus, ActivityTargetKind,
    ActivityTree,
};
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::message::Message;
use std::collections::HashMap;

use super::{ActivityProjector, ChildSessionProjection};

pub const SESSION_EVENTS_CAP: usize = 10_000;

#[derive(Debug, Clone, Default)]
pub struct ActivityApplyOutcome {
    pub rebuild: bool,
    pub bumped: bool,
}

#[derive(Debug, Clone)]
pub struct ActivityProjectionState {
    pub tree: ActivityTree,
    pub events: Vec<EventEnvelope>,
    pub messages: Vec<Message>,
    pub children: HashMap<String, ChildSessionProjection>,
}

impl ActivityProjectionState {
    pub fn new() -> Self {
        Self {
            tree: ActivityTree::default(),
            events: Vec::new(),
            messages: Vec::new(),
            children: HashMap::new(),
        }
    }

    pub fn rebuild(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.tree = ActivityProjector::project_with_children(
            &self.events,
            &self.messages,
            &self.children,
        );
    }

    pub fn push_event(&mut self, event: EventEnvelope) {
        self.events.push(event);
        if self.events.len() > SESSION_EVENTS_CAP {
            let drop = self.events.len() - SESSION_EVENTS_CAP;
            self.events.drain(0..drop);
        }
    }

    pub fn truncate_on_compaction(&mut self) {
        if self.events.len() > SESSION_EVENTS_CAP {
            let drop = self.events.len() - SESSION_EVENTS_CAP;
            self.events.drain(0..drop);
        }
    }

    /// Apply one SSE event to the projection state (incremental patch or full rebuild flag).
    pub fn apply_event(&mut self, event: &EventEnvelope) -> ActivityApplyOutcome {
        let mut rebuild = false;
        let mut bumped = false;
        self.push_event(event.clone());

        if event.kind == EventKind::ToolProgress {
            if let (Some(id), Some(chunk)) = (
                event.payload["tool_call_id"].as_str(),
                event.payload["chunk"].as_str(),
            ) {
                let stream = event.payload["stream"].as_str().unwrap_or("stdout");
                let preview = format!("[{stream}] {chunk}");
                if patch_running_tool_leaf_preview(&mut self.tree, id, &preview) {
                    bumped = true;
                }
            }
        } else if event.kind == EventKind::MessageDelta {
            let delta = event.payload["delta"].as_str().unwrap_or("");
            let message_id = event
                .payload
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !delta.is_empty()
                && !message_id.is_empty()
                && patch_assistant_message_delta(&mut self.tree, message_id, delta)
            {
                bumped = true;
            }
        } else if event.kind == EventKind::MessageCompleted {
            let message_id = event.payload["message_id"].as_str().unwrap_or("");
            let content = event.payload["content"].as_str().unwrap_or("");
            let has_tool_calls = event.payload["has_tool_calls"].as_bool().unwrap_or(false);
            if !message_id.is_empty()
                && patch_assistant_message_completed(
                    &mut self.tree,
                    message_id,
                    content,
                    has_tool_calls,
                )
            {
                bumped = true;
            }
        } else if event.kind == EventKind::ToolStarted {
            let tool_call_id = event.payload["tool_call_id"].as_str().unwrap_or("");
            let tool_name = event.payload["tool_name"].as_str().unwrap_or("tool");
            let title = event.payload.get("title").and_then(|v| v.as_str());
            if patch_tool_started(&mut self.tree, tool_call_id, tool_name, title) {
                bumped = true;
            } else {
                rebuild = true;
            }
        } else if event.kind == EventKind::ToolCompleted {
            let tool_call_id = event.payload["tool_call_id"].as_str().unwrap_or("");
            let tool_name = event.payload["tool_name"].as_str().unwrap_or("tool");
            let title = event.payload.get("title").and_then(|v| v.as_str());
            let failed = event.payload["output"]
                .get("success")
                .and_then(|v| v.as_bool())
                == Some(false);
            if patch_tool_completed(
                &mut self.tree,
                tool_call_id,
                tool_name,
                title,
                failed,
            ) {
                bumped = true;
            } else {
                rebuild = true;
            }
        } else if event_requires_full_rebuild(event.kind) {
            rebuild = true;
        }

        if rebuild {
            self.rebuild();
            bumped = true;
        }

        ActivityApplyOutcome { rebuild, bumped }
    }
}

/// Whether an event should trigger a full activity-tree projection.
pub fn event_requires_full_rebuild(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::ProviderTurnStarted
            | EventKind::SessionIdle
            | EventKind::SessionInterrupted
            | EventKind::SessionCompacted
            | EventKind::TodoUpdated
            | EventKind::PolicyPrompted
            | EventKind::PolicyResolved
            | EventKind::QuestionRequested
            | EventKind::PlanCompleted
            | EventKind::ActivityUpdated
    )
}

pub fn patch_tool_started(
    tree: &mut ActivityTree,
    tool_call_id: &str,
    tool_name: &str,
    title: Option<&str>,
) -> bool {
    if tool_call_id.is_empty() {
        return false;
    }
    if find_tool_leaf(&mut tree.roots, tool_call_id).is_some() {
        return false;
    }
    let kind = tool_activity_kind(tool_name);
    let leaf = ActivityLeaf {
        id: tool_call_id.to_string(),
        kind,
        verb: tool_activity_verb(tool_name, title),
        target: ActivityTargetKind::None,
        meta: ActivityMeta::default(),
        status: ActivityStatus::Running,
        detail_ref: Some(tool_call_id.to_string()),
        text: title.map(String::from),
    };
    push_tool_leaf_to_active_turn(tree, leaf)
}

pub fn patch_tool_completed(
    tree: &mut ActivityTree,
    tool_call_id: &str,
    tool_name: &str,
    title: Option<&str>,
    failed: bool,
) -> bool {
    if tool_call_id.is_empty() {
        return false;
    }
    let Some(leaf) = find_tool_leaf(&mut tree.roots, tool_call_id) else {
        return patch_tool_started(tree, tool_call_id, tool_name, title);
    };
    leaf.kind = tool_activity_kind(tool_name);
    leaf.verb = tool_activity_verb(tool_name, title);
    leaf.status = if failed {
        ActivityStatus::Failed
    } else {
        ActivityStatus::Completed
    };
    if title.is_some() {
        leaf.text = title.map(String::from);
    }
    true
}

pub fn patch_assistant_message_completed(
    tree: &mut ActivityTree,
    message_id: &str,
    content: &str,
    has_tool_calls: bool,
) -> bool {
    fn walk(
        nodes: &mut [ActivityNode],
        message_id: &str,
        content: &str,
        has_tool_calls: bool,
    ) -> bool {
        for node in nodes {
            match node {
                ActivityNode::Leaf(leaf)
                    if leaf.kind == ActivityKind::AssistantMessage && leaf.id == message_id =>
                {
                    leaf.text = Some(content.to_string());
                    leaf.status = if has_tool_calls {
                        ActivityStatus::Running
                    } else {
                        ActivityStatus::Completed
                    };
                    return true;
                }
                ActivityNode::Group(group) => {
                    if walk(&mut group.children, message_id, content, has_tool_calls) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    walk(&mut tree.roots, message_id, content, has_tool_calls)
}

pub fn patch_assistant_message_delta(
    tree: &mut ActivityTree,
    message_id: &str,
    delta: &str,
) -> bool {
    if delta.is_empty() {
        return false;
    }
    fn walk(nodes: &mut [ActivityNode], message_id: &str, delta: &str) -> bool {
        for node in nodes {
            match node {
                ActivityNode::Leaf(leaf)
                    if leaf.kind == ActivityKind::AssistantMessage
                        && leaf.id == message_id
                        && leaf.status == ActivityStatus::Running =>
                {
                    let mut text = leaf.text.take().unwrap_or_default();
                    text.push_str(delta);
                    leaf.text = Some(text);
                    return true;
                }
                ActivityNode::Group(group) => {
                    if walk(&mut group.children, message_id, delta) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    if walk(&mut tree.roots, message_id, delta) {
        return true;
    }
    tree.roots.push(ActivityNode::Leaf(ActivityLeaf {
        id: message_id.to_string(),
        kind: ActivityKind::AssistantMessage,
        verb: "Assistant".into(),
        target: ActivityTargetKind::None,
        meta: ActivityMeta::default(),
        status: ActivityStatus::Running,
        detail_ref: None,
        text: Some(delta.to_string()),
    }));
    true
}

pub fn patch_running_tool_leaf_preview(
    tree: &mut ActivityTree,
    leaf_id: &str,
    preview: &str,
) -> bool {
    fn walk(nodes: &mut [ActivityNode], leaf_id: &str, preview: &str) -> bool {
        for node in nodes {
            match node {
                ActivityNode::Leaf(leaf)
                    if leaf.id == leaf_id && leaf.status == ActivityStatus::Running =>
                {
                    leaf.text = Some(preview.to_string());
                    return true;
                }
                ActivityNode::Group(group) => {
                    if walk(&mut group.children, leaf_id, preview) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    walk(&mut tree.roots, leaf_id, preview)
}

fn tool_activity_kind(tool_name: &str) -> ActivityKind {
    match tool_name {
        "read" => ActivityKind::Read,
        "write" => ActivityKind::Write,
        "edit" => ActivityKind::Edit,
        "apply_patch" => ActivityKind::ApplyPatch,
        "grep" => ActivityKind::Grep,
        "glob" => ActivityKind::Glob,
        "list" => ActivityKind::List,
        "bash" => ActivityKind::Bash,
        "webfetch" | "web_fetch" => ActivityKind::WebFetch,
        "websearch" | "web_search" => ActivityKind::WebSearch,
        "skill" => ActivityKind::Skill,
        "lsp" => ActivityKind::Lsp,
        "todo_write" => ActivityKind::TodoWrite,
        "todo_read" => ActivityKind::TodoRead,
        "task" => ActivityKind::Task,
        "question" => ActivityKind::Question,
        "plan_complete" | "plan.complete" => ActivityKind::PlanReady,
        _ if tool_name.starts_with("mcp_") => ActivityKind::Mcp,
        _ => ActivityKind::Bash,
    }
}

fn tool_activity_verb(tool_name: &str, title: Option<&str>) -> String {
    title
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| match tool_name {
            "read" => "Read".into(),
            "write" => "Write".into(),
            "edit" => "Edit".into(),
            "bash" => "Run command".into(),
            "grep" => "Search".into(),
            other => other.to_string(),
        })
}

fn push_tool_leaf_to_active_turn(tree: &mut ActivityTree, leaf: ActivityLeaf) -> bool {
    if let Some(ActivityNode::Group(group)) = tree.roots.last_mut() {
        group.children.push(ActivityNode::Leaf(leaf));
        return true;
    }
    tree.roots.push(ActivityNode::Leaf(leaf));
    true
}

fn find_tool_leaf<'a>(
    nodes: &'a mut [ActivityNode],
    tool_call_id: &str,
) -> Option<&'a mut ActivityLeaf> {
    for node in nodes {
        match node {
            ActivityNode::Leaf(leaf) if leaf.id == tool_call_id => return Some(leaf),
            ActivityNode::Group(group) => {
                if let Some(leaf) = find_tool_leaf(&mut group.children, tool_call_id) {
                    return Some(leaf);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_schema::event::EventEnvelope;
    use chrono::Utc;

    fn ev(kind: EventKind, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            seq: 1,
            kind,
            session_id: None,
            payload,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn message_delta_patches_without_full_rebuild() {
        let mut state = ActivityProjectionState::new();
        let outcome = state.apply_event(&ev(
            EventKind::MessageDelta,
            serde_json::json!({ "message_id": "m1", "delta": "Hi" }),
        ));
        assert!(!outcome.rebuild);
        assert!(outcome.bumped);
    }

    #[test]
    fn turn_boundary_triggers_rebuild() {
        let mut state = ActivityProjectionState::new();
        state.apply_event(&ev(
            EventKind::MessageDelta,
            serde_json::json!({ "message_id": "m1", "delta": "Hi" }),
        ));
        let outcome = state.apply_event(&ev(EventKind::ProviderTurnStarted, serde_json::json!({})));
        assert!(outcome.rebuild);
    }
}
