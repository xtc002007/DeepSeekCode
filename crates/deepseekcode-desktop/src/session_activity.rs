//! Session activity projection state extracted from WorkspaceView.

use deepseekcode_schema::activity::ActivityTree;
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::message::Message;
use deepseekcode_sdk::ChildSessionProjection;
use deepseekcode_sdk::{
    patch_assistant_message_completed, patch_assistant_message_delta, patch_running_tool_leaf_preview,
    patch_tool_completed, patch_tool_started, event_requires_full_rebuild, ActivityProjector,
    SESSION_EVENTS_CAP,
};
use std::collections::{HashMap, HashSet};

use crate::activity_timeline::{DetailStore, rebuild_detail_store};

pub fn push_session_event(events: &mut Vec<EventEnvelope>, event: EventEnvelope) {
    events.push(event);
    if events.len() > SESSION_EVENTS_CAP {
        let drop = events.len() - SESSION_EVENTS_CAP;
        events.drain(0..drop);
    }
}

pub fn truncate_session_events_on_compaction(events: &mut Vec<EventEnvelope>) {
    if events.len() > SESSION_EVENTS_CAP {
        let drop = events.len() - SESSION_EVENTS_CAP;
        events.drain(0..drop);
    }
}

/// Event-sourced session activity projection owned by the workspace shell.
#[derive(Debug, Clone)]
pub struct SessionActivityStore {
    pub session_events: Vec<EventEnvelope>,
    pub session_messages: Vec<Message>,
    pub activity_tree: ActivityTree,
    pub child_session_projections: HashMap<String, ChildSessionProjection>,
    pub detail_store: DetailStore,
    pub expanded_activity_groups: HashSet<String>,
    pub collapsed_activity_groups: HashSet<String>,
    pub expanded_activity_leaves: HashSet<String>,
    pub timeline_render_generation: u64,
    pub session_events_enabled: bool,
    pub ignore_stale_run_events: bool,
    pub since_seq: i64,
}

impl Default for SessionActivityStore {
    fn default() -> Self {
        Self {
            session_events: Vec::new(),
            session_messages: Vec::new(),
            activity_tree: ActivityTree::default(),
            child_session_projections: HashMap::new(),
            detail_store: DetailStore::default(),
            expanded_activity_groups: HashSet::new(),
            collapsed_activity_groups: HashSet::new(),
            expanded_activity_leaves: HashSet::new(),
            timeline_render_generation: 0,
            session_events_enabled: false,
            ignore_stale_run_events: false,
            since_seq: 0,
        }
    }
}

#[derive(Default)]
pub struct ActivityEventOutcome {
    pub rebuild_activity: bool,
    pub timeline_bumped: bool,
}

impl SessionActivityStore {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn rebuild_activity_tree(&mut self, reason: &str) {
        let event_count = self.session_events.len();
        let roots_before = self.activity_tree.roots.len();
        if event_count == 0 {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                reason,
                "activity tree rebuild skipped (no session events)"
            );
            return;
        }

        let new_tree = ActivityProjector::project_with_children(
            &self.session_events,
            &self.session_messages,
            &self.child_session_projections,
        );
        let roots_after = new_tree.roots.len();

        if roots_after == 0 && roots_before > 0 {
            tracing::warn!(
                target: deepseekcode_telemetry::DESKTOP,
                reason,
                event_count,
                roots_before,
                "activity tree rebuild returned empty roots; keeping previous tree"
            );
            return;
        }

        self.activity_tree = new_tree;

        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            reason,
            event_count,
            roots_before,
            roots_after,
            "activity tree rebuilt"
        );
    }

    pub fn apply_session_event(&mut self, event: &EventEnvelope) -> ActivityEventOutcome {
        if event.kind == EventKind::ActivityUpdated {
            return ActivityEventOutcome::default();
        }
        let mut rebuild_activity = false;
        let mut timeline_bumped = false;
        push_session_event(&mut self.session_events, event.clone());

        if event.kind == EventKind::ToolProgress {
            if let (Some(id), Some(chunk)) = (
                event.payload["tool_call_id"].as_str(),
                event.payload["chunk"].as_str(),
            ) {
                let stream = event.payload["stream"].as_str().unwrap_or("stdout");
                self.detail_store.append_output(id, chunk, stream);
                let preview = format!("[{stream}] {chunk}");
                if patch_running_tool_leaf_preview(&mut self.activity_tree, id, &preview) {
                    timeline_bumped = true;
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
                && patch_assistant_message_delta(&mut self.activity_tree, message_id, delta)
            {
                timeline_bumped = true;
            }
        } else if event.kind == EventKind::MessageCompleted {
            let message_id = event.payload["message_id"].as_str().unwrap_or("");
            let content = event.payload["content"].as_str().unwrap_or("");
            let has_tool_calls = event.payload["has_tool_calls"].as_bool().unwrap_or(false);
            if !message_id.is_empty()
                && patch_assistant_message_completed(
                    &mut self.activity_tree,
                    message_id,
                    content,
                    has_tool_calls,
                )
            {
                timeline_bumped = true;
            }
        } else if event.kind == EventKind::ToolStarted {
            let tool_call_id = event.payload["tool_call_id"].as_str().unwrap_or("");
            let tool_name = event.payload["tool_name"].as_str().unwrap_or("tool");
            let title = event.payload.get("title").and_then(|v| v.as_str());
            if patch_tool_started(&mut self.activity_tree, tool_call_id, tool_name, title) {
                timeline_bumped = true;
            } else {
                rebuild_activity = true;
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
                &mut self.activity_tree,
                tool_call_id,
                tool_name,
                title,
                failed,
            ) {
                timeline_bumped = true;
            } else {
                rebuild_activity = true;
            }
        } else if event_requires_full_rebuild(event.kind) {
            rebuild_activity = true;
        }

        ActivityEventOutcome {
            rebuild_activity,
            timeline_bumped,
        }
    }

    pub fn hydrate_from_api(
        &mut self,
        session_events: Vec<EventEnvelope>,
        session_messages: Vec<Message>,
        activity_tree: ActivityTree,
        detail_store: DetailStore,
        resume_seq: i64,
    ) {
        self.since_seq = resume_seq;
        self.session_events_enabled = true;
        self.ignore_stale_run_events = true;
        if !session_events.is_empty() {
            self.session_events = session_events;
        }
        if !session_messages.is_empty() {
            self.session_messages = session_messages;
        }
        self.detail_store = detail_store;

        if activity_tree.roots.is_empty() && !self.activity_tree.roots.is_empty() {
            if !self.session_events.is_empty() {
                self.rebuild_activity_tree("hydrate_from_api_fallback");
            }
            return;
        }

        self.activity_tree = activity_tree;
    }

    pub fn rebuild_detail_from_events(&mut self) {
        self.detail_store = rebuild_detail_store(&self.session_events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_schema::event::EventKind;
    use deepseekcode_types::SessionId;

    #[test]
    fn apply_message_delta_bumps_generation_without_full_rebuild() {
        let mut store = SessionActivityStore::default();
        store.session_events_enabled = true;
        store.activity_tree = ActivityTree {
            roots: vec![deepseekcode_schema::activity::ActivityNode::Leaf(
                deepseekcode_schema::activity::ActivityLeaf {
                    id: "msg-1".into(),
                    kind: deepseekcode_schema::activity::ActivityKind::AssistantMessage,
                    verb: "Assistant".into(),
                    target: deepseekcode_schema::activity::ActivityTargetKind::None,
                    meta: deepseekcode_schema::activity::ActivityMeta::default(),
                    status: deepseekcode_schema::activity::ActivityStatus::Running,
                    detail_ref: None,
                    text: Some(String::new()),
                },
            )],
        };
        let event = EventEnvelope {
            seq: 1,
            kind: EventKind::MessageDelta,
            session_id: Some(SessionId::new()),
            payload: serde_json::json!({
                "message_id": "msg-1",
                "delta": "hello"
            }),
            created_at: chrono::Utc::now(),
        };
        let outcome = store.apply_session_event(&event);
        assert!(!outcome.rebuild_activity);
        assert!(outcome.timeline_bumped);
        assert_eq!(store.session_events.len(), 1);
    }

    #[test]
    fn tool_started_patches_without_full_rebuild() {
        let mut store = SessionActivityStore::default();
        store.session_events_enabled = true;
        let event = EventEnvelope {
            seq: 2,
            kind: EventKind::ToolStarted,
            session_id: Some(SessionId::new()),
            payload: serde_json::json!({
                "tool_call_id": "tc-1",
                "tool_name": "read",
                "title": "Read README.md"
            }),
            created_at: chrono::Utc::now(),
        };
        let outcome = store.apply_session_event(&event);
        assert!(!outcome.rebuild_activity);
        assert!(outcome.timeline_bumped);
        assert_eq!(store.activity_tree.roots.len(), 1);
    }

    #[test]
    fn session_idle_triggers_full_rebuild() {
        let mut store = SessionActivityStore::default();
        store.session_events_enabled = true;
        let event = EventEnvelope {
            seq: 3,
            kind: EventKind::SessionIdle,
            session_id: Some(SessionId::new()),
            payload: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        let outcome = store.apply_session_event(&event);
        assert!(outcome.rebuild_activity);
    }

    #[test]
    fn hydrate_from_api_keeps_tree_when_api_returns_empty() {
        let mut store = SessionActivityStore::default();
        store.activity_tree = ActivityTree {
            roots: vec![deepseekcode_schema::activity::ActivityNode::Leaf(
                deepseekcode_schema::activity::ActivityLeaf {
                    id: "msg-1".into(),
                    kind: deepseekcode_schema::activity::ActivityKind::AssistantMessage,
                    verb: "Assistant".into(),
                    target: deepseekcode_schema::activity::ActivityTargetKind::None,
                    meta: deepseekcode_schema::activity::ActivityMeta::default(),
                    status: deepseekcode_schema::activity::ActivityStatus::Completed,
                    detail_ref: None,
                    text: Some("done".into()),
                },
            )],
        };
        store.hydrate_from_api(
            Vec::new(),
            Vec::new(),
            ActivityTree::default(),
            DetailStore::default(),
            0,
        );

        assert_eq!(store.activity_tree.roots.len(), 1);
    }
}
