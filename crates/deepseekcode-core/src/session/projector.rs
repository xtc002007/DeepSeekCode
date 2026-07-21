//! Projects durable events into client-consumable timeline entries and message views.

use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::message::{Message, MessageRole};
use deepseekcode_storage::{message_visible_to_runner, EventStore, SessionStore};
use deepseekcode_types::{MessageId, Result, SessionId};
use std::str::FromStr;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelineEntry {
    pub seq: i64,
    pub kind: String,
    pub label: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

pub struct EventProjector;

impl EventProjector {
    pub fn from_event(event: &EventEnvelope) -> TimelineEntry {
        let (label, detail) = match event.kind {
            EventKind::SessionCreated => ("Session".into(), "Session created".into()),
            EventKind::PromptAdmitted => {
                let prompt = event.payload["prompt"].as_str().unwrap_or("");
                ("User".into(), prompt.to_string())
            }
            EventKind::Prompted => ("User".into(), "Prompt promoted".into()),
            EventKind::MessageDelta => {
                let delta = event.payload["delta"].as_str().unwrap_or("");
                ("Assistant".into(), delta.to_string())
            }
            EventKind::MessageCompleted => ("Assistant".into(), "Message completed".into()),
            EventKind::ToolStarted => {
                let name = event.payload["tool_name"].as_str().unwrap_or("tool");
                ("Tool".into(), format!("Running {name}"))
            }
            EventKind::ToolCompleted => {
                let name = event.payload["tool_name"].as_str().unwrap_or("tool");
                ("Tool".into(), format!("Completed {name}"))
            }
            EventKind::PermissionRequested => ("Permission".into(), "Permission required".into()),
            EventKind::PolicyPrompted => {
                let scope = event.payload["scope"].as_str().unwrap_or("tool");
                let tool = event.payload["tool_name"].as_str().unwrap_or("tool");
                ("Policy".into(), format!("{scope} · {tool}"))
            }
            EventKind::ContextUpdated => ("Context".into(), "Context updated".into()),
            EventKind::SessionCompacted => ("Session".into(), "Context compacted".into()),
            EventKind::SessionInterrupted => ("Session".into(), "Interrupted".into()),
            EventKind::SessionIdle => ("Session".into(), "Idle".into()),
            EventKind::ProviderTurnStarted => ("Provider".into(), "Turn started".into()),
            EventKind::McpStatusChanged => ("MCP".into(), "MCP status changed".into()),
            EventKind::RevertCommitted => ("Session".into(), "Revert committed".into()),
            EventKind::TodoUpdated => ("Todo".into(), "Todo list updated".into()),
            EventKind::ToolProgress => ("Tool".into(), "Tool progress".into()),
            EventKind::PermissionResolved => ("Permission".into(), "Permission resolved".into()),
            EventKind::PolicyResolved => {
                let reply = event.payload["reply"].as_str().unwrap_or("resolved");
                ("Policy".into(), format!("Policy {reply}"))
            }
            EventKind::QuestionRequested => ("Question".into(), "Question asked".into()),
            EventKind::PlanCompleted => {
                let title = event.payload["title"].as_str().unwrap_or("Plan");
                ("Plan".into(), format!("Plan completed: {title}"))
            }
            EventKind::ActivityUpdated => ("Activity".into(), "Activity updated".into()),
        };
        let message_id = match event.kind {
            EventKind::Prompted | EventKind::MessageCompleted => event
                .payload
                .get("message_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            EventKind::PromptAdmitted => event
                .payload
                .get("message_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            _ => None,
        };
        TimelineEntry {
            seq: event.seq,
            kind: event.kind.as_str().to_string(),
            label,
            detail,
            message_id,
        }
    }

    pub async fn project_events(
        store: &EventStore,
        session_id: SessionId,
        since_seq: i64,
    ) -> Result<Vec<TimelineEntry>> {
        let events = store.tail(since_seq, 10_000).await?;
        Ok(events
            .into_iter()
            .filter(|e| e.session_id == Some(session_id))
            .map(|e| Self::from_event(&e))
            .collect())
    }

    /// Projects visible messages by reconciling durable store rows with the event log.
    pub async fn project_messages(
        sessions: &SessionStore,
        events: &EventStore,
        session_id: SessionId,
    ) -> Result<Vec<Message>> {
        let mut messages = sessions.list_messages(session_id).await?;
        let session_events = Self::session_events(events, session_id).await?;
        if session_events.is_empty() {
            return Ok(messages);
        }

        let prompted = Self::prompted_message_ids(&session_events);

        if !prompted.is_empty() {
            messages.retain(|m| match m.role {
                MessageRole::User => prompted.contains(&m.id),
                _ => true,
            });
        }

        // Always order by durable message sequence. Event-log seq values live on a
        // different scale and can place tool rows before their assistant tool_calls.
        messages.sort_by_key(|m| m.seq);

        Ok(messages)
    }

    pub async fn project_messages_for_runner(
        sessions: &SessionStore,
        events: &EventStore,
        session_id: SessionId,
        baseline_seq: i64,
    ) -> Result<Vec<Message>> {
        let compaction_seq = sessions.latest_compaction_seq(session_id).await?;
        let messages = Self::project_messages(sessions, events, session_id).await?;
        Ok(messages
            .into_iter()
            .filter(|m| message_visible_to_runner(m, baseline_seq, compaction_seq))
            .collect())
    }

    pub fn message_role_label(role: MessageRole) -> &'static str {
        match role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
            MessageRole::Compaction => "Compaction",
        }
    }

    async fn session_events(
        store: &EventStore,
        session_id: SessionId,
    ) -> Result<Vec<EventEnvelope>> {
        Ok(store
            .tail(0, 100_000)
            .await?
            .into_iter()
            .filter(|e| e.session_id == Some(session_id))
            .collect())
    }

    fn prompted_message_ids(events: &[EventEnvelope]) -> std::collections::HashSet<MessageId> {
        events
            .iter()
            .filter(|e| e.kind == EventKind::Prompted)
            .filter_map(|e| e.payload.get("message_id").and_then(|v| v.as_str()))
            .filter_map(|s| MessageId::from_str(s).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_schema::event::{EventEnvelope, EventKind};
    use deepseekcode_storage::Database;
    use std::sync::Arc;

    #[tokio::test]
    async fn project_messages_requires_prompted_event_for_user_rows() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let pool = db.pool().clone();
        let sessions = SessionStore::new(pool.clone());
        let events = EventStore::new(pool);
        let session_id = SessionId::new();

        sessions
            .create(
                session_id,
                deepseekcode_types::LocationId::new(),
                "/tmp",
                deepseekcode_schema::agent::AgentKind::Build,
                None,
                None,
            )
            .await
            .unwrap();

        let orphan_id = MessageId::new();
        sessions
            .insert_message(orphan_id, session_id, "user", "orphan")
            .await
            .unwrap();

        let promoted_id = MessageId::new();
        sessions
            .insert_message(promoted_id, session_id, "user", "visible")
            .await
            .unwrap();
        events
            .append(
                EventKind::Prompted,
                Some(session_id),
                serde_json::json!({ "message_id": promoted_id.to_string() }),
            )
            .await
            .unwrap();

        let projected = EventProjector::project_messages(&sessions, &events, session_id)
            .await
            .unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, promoted_id);
    }

    #[tokio::test]
    async fn project_messages_keeps_tool_results_after_assistant_tool_calls() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let pool = db.pool().clone();
        let sessions = SessionStore::new(pool.clone());
        let events = EventStore::new(pool);
        let session_id = SessionId::new();

        sessions
            .create(
                session_id,
                deepseekcode_types::LocationId::new(),
                "/tmp",
                deepseekcode_schema::agent::AgentKind::Build,
                None,
                None,
            )
            .await
            .unwrap();

        let user_id = MessageId::new();
        sessions
            .insert_message(user_id, session_id, "user", "hello")
            .await
            .unwrap();
        events
            .append(
                EventKind::Prompted,
                Some(session_id),
                serde_json::json!({ "message_id": user_id.to_string() }),
            )
            .await
            .unwrap();

        let assistant_id = MessageId::new();
        sessions
            .insert_assistant_turn(assistant_id, session_id, None, "thinking")
            .await
            .unwrap();
        sessions
            .append_tool_call(
                assistant_id,
                1,
                "call_test",
                "read",
                &serde_json::json!({ "path": "README.md" }),
            )
            .await
            .unwrap();
        events
            .append(
                EventKind::MessageCompleted,
                Some(session_id),
                serde_json::json!({ "message_id": assistant_id.to_string() }),
            )
            .await
            .unwrap();

        let tool_id = MessageId::new();
        sessions
            .insert_tool_result(
                tool_id,
                session_id,
                0,
                "call_test",
                "read",
                &serde_json::json!({ "content": "ok" }),
            )
            .await
            .unwrap();

        let projected = EventProjector::project_messages(&sessions, &events, session_id)
            .await
            .unwrap();
        assert_eq!(projected.len(), 3);
        assert_eq!(projected[0].role, MessageRole::User);
        assert_eq!(projected[1].role, MessageRole::Assistant);
        assert_eq!(projected[2].role, MessageRole::Tool);
    }

    #[test]
    fn timeline_entry_includes_message_id_from_prompted_event() {
        let event = EventEnvelope {
            seq: 1,
            kind: EventKind::Prompted,
            session_id: Some(SessionId::new()),
            payload: serde_json::json!({ "message_id": "01HZTESTMESSAGEID00" }),
            created_at: chrono::Utc::now(),
        };
        let entry = EventProjector::from_event(&event);
        assert_eq!(entry.message_id.as_deref(), Some("01HZTESTMESSAGEID00"));
    }
}
