use chrono::Utc;
use deepseekcode_core::activity::ActivityProjector;
use deepseekcode_schema::activity::{ActivityGroupKind, ActivityKind, ActivityNode};
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::todo::TodoItem;
use serde_json::json;

fn ev(kind: EventKind, payload: serde_json::Value) -> EventEnvelope {
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
fn turn_summary_counts_edits_and_searches() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "e1",
                "tool_name": "edit",
                "metadata": { "path": "a.rs", "diff_stats": { "added": 21, "removed": 1 } },
                "duration_ms": 5
            }),
        ),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "g1",
                "tool_name": "grep",
                "metadata": { "pattern": "foo", "match_count": 3 },
                "duration_ms": 2
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let group = tree
        .roots
        .iter()
        .find_map(|n| match n {
            ActivityNode::Group(g) if g.kind == ActivityGroupKind::ProviderTurn => Some(g),
            _ => None,
        })
        .expect("provider turn group");
    let summary = group.summary.as_ref().expect("summary");
    assert_eq!(summary.files_edited, 1);
    assert_eq!(summary.searches, 1);
    assert_eq!(summary.lines_added, 21);
    assert_eq!(summary.lines_removed, 1);
}

#[test]
fn todo_updated_inserts_todo_leaf_with_focus_index() {
    let todos = vec![TodoItem {
        content: "Ship feature".into(),
        status: "pending".into(),
        priority: "high".into(),
    }];
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(EventKind::TodoUpdated, json!({ "todos": todos })),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g
                .children
                .iter()
                .find_map(|c| match c {
                    ActivityNode::Leaf(l) if l.kind == ActivityKind::TodoWrite => Some(l),
                    _ => None,
                }),
            _ => None,
        })
        .next()
        .expect("todo leaf");
    assert_eq!(leaf.verb, "Updated todos");
    assert_eq!(leaf.meta.todo_focus_index, Some(0));
}

#[test]
fn tool_started_and_completed_merge_to_single_leaf() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolStarted,
            json!({
                "tool_call_id": "r1",
                "tool_name": "read",
                "title": "Read foo.rs",
                "metadata": { "path": "foo.rs", "line_start": 1, "line_end": 10 }
            }),
        ),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "r1",
                "tool_name": "read",
                "metadata": { "path": "foo.rs", "line_start": 1, "line_end": 10 },
                "duration_ms": 3
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let read_leaves = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => Some(
                g.children
                    .iter()
                    .filter(|c| {
                        matches!(
                            c,
                            ActivityNode::Leaf(l) if l.kind == ActivityKind::Read
                        )
                    })
                    .count(),
            ),
            _ => None,
        })
        .sum::<usize>();
    assert_eq!(read_leaves, 1);
}

#[test]
fn list_tool_leaf_uses_entry_count_metadata() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "l1",
                "tool_name": "list",
                "metadata": { "path": "src", "entry_count": 7 },
                "duration_ms": 1
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::List => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("list leaf");
    assert_eq!(leaf.meta.entry_count, Some(7));
}

#[test]
fn websearch_uses_result_count_metadata() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "w1",
                "tool_name": "websearch",
                "metadata": { "result_count": 4, "first_url": "https://example.com" },
                "duration_ms": 1
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::WebSearch => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("websearch leaf");
    assert_eq!(leaf.meta.match_count, Some(4));
    assert!(matches!(
        leaf.target,
        deepseekcode_schema::activity::ActivityTargetKind::Url { .. }
    ));
}

#[test]
fn task_subagent_nests_child_session_events() {
    let parent_events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "t1",
                "tool_name": "task",
                "output": {
                    "title": "Task",
                    "output": { "session_id": "child-1", "agent": "build", "status": "spawned" },
                    "metadata": {},
                    "duration_ms": 1
                }
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let child_events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "r1",
                "tool_name": "read",
                "metadata": { "path": "lib.rs", "line_start": 1, "line_end": 3 },
                "duration_ms": 1
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let mut children = std::collections::HashMap::new();
    children.insert(
        "child-1".into(),
        deepseekcode_core::ChildSessionProjection {
            events: child_events,
            messages: vec![],
        },
    );
    let tree = ActivityProjector::project_with_children(&parent_events, &[], &children);
    let group = tree
        .roots
        .iter()
        .find_map(|n| match n {
            ActivityNode::Group(g) if g.kind == ActivityGroupKind::ProviderTurn => g
                .children
                .iter()
                .find_map(|c| match c {
                    ActivityNode::Group(sg) if sg.kind == ActivityGroupKind::TaskSubagent => Some(sg),
                    _ => None,
                }),
            _ => None,
        })
        .expect("task group");
        assert!(group.children.len() >= 2);
}

#[test]
fn plan_complete_projects_plan_ready_leaf() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "p1",
                "tool_name": "plan_complete",
                "input": { "title": "Auth refactor" },
                "output": {
                    "title": "Plan complete",
                    "output": {
                        "plan_id": "auth-plan",
                        "plan_path": ".deepseekcode/plan/auth-plan/PLAN.md",
                        "title": "Auth refactor",
                        "status": "completed"
                    },
                    "metadata": {},
                    "duration_ms": 1
                }
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .find_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::PlanReady => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .expect("plan ready leaf");
    assert_eq!(
        leaf.meta.first_match_path.as_deref(),
        Some(".deepseekcode/plan/auth-plan/PLAN.md")
    );
}

#[test]
fn permission_resolved_shows_allowed_and_denied_verbs() {
    let allowed_events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::PermissionResolved,
            json!({ "tool_name": "bash", "status": "allowed" }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&allowed_events, &[]);
    let allowed_leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::Permission => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("permission leaf");
    assert!(allowed_leaf.verb.contains("Allowed"));

    let denied_events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::PermissionResolved,
            json!({ "tool_name": "write", "decision": "denied" }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&denied_events, &[]);
    let denied_leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::Permission => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("permission leaf");
    assert!(denied_leaf.verb.contains("Denied"));
}

#[test]
fn assistant_text_delta_accumulates_until_message_completed() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::MessageDelta,
            json!({ "message_id": "a1", "delta": "Hello " }),
        ),
        ev(
            EventKind::MessageDelta,
            json!({ "message_id": "a1", "delta": "world" }),
        ),
        ev(
            EventKind::MessageCompleted,
            json!({
                "message_id": "a1",
                "content": "Hello world",
                "has_tool_calls": false
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::AssistantMessage => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("assistant leaf");
    assert_eq!(leaf.text.as_deref(), Some("Hello world"));
}

#[test]
fn message_completed_prefers_buffered_stream_when_event_content_empty() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::MessageDelta,
            json!({ "message_id": "a1", "delta": "Full streamed analysis body." }),
        ),
        ev(
            EventKind::MessageCompleted,
            json!({
                "message_id": "a1",
                "content": "",
                "has_tool_calls": false
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::AssistantMessage => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("assistant leaf");
    assert_eq!(
        leaf.text.as_deref(),
        Some("Full streamed analysis body.")
    );
}

#[test]
fn user_messages_appear_before_session_meta_not_at_end() {
    use deepseekcode_schema::message::{Message, MessagePart, MessageRole, PartKind};
    use deepseekcode_types::{MessageId, SessionId};

    let user_id = MessageId::new();
    let events = vec![
        ev(
            EventKind::PromptAdmitted,
            json!({
                "message_id": user_id.to_string(),
            }),
        ),
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::MessageCompleted,
            json!({
                "message_id": "assistant-1",
                "content": "Analysis result.",
                "has_tool_calls": false
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let messages = vec![Message {
        id: user_id,
        session_id: SessionId::new(),
        role: MessageRole::User,
        parts: vec![MessagePart {
            kind: PartKind::Text,
            text: Some("大体分析这个项目".into()),
            tool_call_id: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
        }],
        seq: 1,
        created_at: Utc::now(),
    }];
    let tree = ActivityProjector::project(&events, &messages);
    let user_idx = tree
        .roots
        .iter()
        .position(|n| matches!(n, ActivityNode::Leaf(l) if l.kind == ActivityKind::UserMessage))
        .expect("user leaf");
    let idle_idx = tree
        .roots
        .iter()
        .position(|n| {
            matches!(
                n,
                ActivityNode::Group(g) if g.kind == ActivityGroupKind::SessionMeta
                    && g.label == "Session idle"
            )
        })
        .expect("session idle");
    assert!(user_idx < idle_idx, "user message should precede session idle");
}

#[test]
fn assistant_message_from_messages_supplements_short_event_content() {
    use deepseekcode_schema::message::{Message, MessagePart, MessageRole, PartKind};
    use deepseekcode_types::{MessageId, SessionId};

    let message_id = MessageId::new();
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::MessageCompleted,
            json!({
                "message_id": message_id.to_string(),
                "content": "Opening line only.",
                "has_tool_calls": false
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let messages = vec![Message {
        id: message_id,
        session_id: SessionId::new(),
        role: MessageRole::Assistant,
        parts: vec![MessagePart {
            kind: PartKind::Text,
            text: Some("Opening line only.\n\nFull analysis body.".into()),
            tool_call_id: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
        }],
        seq: 0,
        created_at: Utc::now(),
    }];
    let tree = ActivityProjector::project(&events, &messages);
    let leaf = tree
        .roots
        .iter()
        .filter_map(|n| match n {
            ActivityNode::Group(g) => g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) if l.kind == ActivityKind::AssistantMessage => Some(l),
                _ => None,
            }),
            _ => None,
        })
        .next()
        .expect("assistant leaf");
    assert!(leaf
        .text
        .as_deref()
        .unwrap_or("")
        .contains("Full analysis body"));
}

fn find_assistant_leaves(nodes: &[ActivityNode], out: &mut Vec<String>) {
    for node in nodes {
        match node {
            ActivityNode::Leaf(leaf) if leaf.kind == ActivityKind::AssistantMessage => {
                if let Some(text) = &leaf.text {
                    out.push(text.clone());
                }
            }
            ActivityNode::Group(group) => find_assistant_leaves(&group.children, out),
            _ => {}
        }
    }
}

#[tokio::test]
async fn project_real_session_preserves_full_final_assistant_text() {
    let db_path = std::env::var("USERPROFILE")
        .map(|home| {
            std::path::PathBuf::from(home)
                .join("AppData")
                .join("Roaming")
                .join("deepseekcode")
                .join("deepseekcode.db")
        })
        .unwrap_or_default();
    if !db_path.exists() {
        eprintln!("skip: db not found at {}", db_path.display());
        return;
    }

    use deepseekcode_storage::{Database, EventStore, SessionStore};
    use deepseekcode_types::SessionId;

    let db = Database::open(&format!("sqlite://{}", db_path.display())).await.unwrap();
    let pool = db.pool().clone();
    let sessions = SessionStore::new(pool.clone());
    let events = EventStore::new(pool);
    let session_id =
        SessionId::from_str("01KXFRRZGGB8ZJKBX6DYABTRAA").expect("session id");

    let evs = events.tail_filtered(0, 10_000, Some(session_id)).await.unwrap();
    let messages = sessions.list_messages(session_id).await.unwrap();
    let tree = ActivityProjector::project(&evs, &messages);

    let mut assistant_texts = Vec::new();
    find_assistant_leaves(&tree.roots, &mut assistant_texts);
    let final_text = assistant_texts
        .iter()
        .max_by_key(|t| t.len())
        .map(String::as_str)
        .unwrap_or("");
    assert!(
        final_text.len() >= 500,
        "expected full final assistant analysis (>=500 chars), got len={} preview={:?}",
        final_text.len(),
        final_text.chars().take(120).collect::<String>()
    );
    assert!(
        final_text.contains("项目分析"),
        "missing analysis heading in projected text: {:?}",
        final_text.chars().take(200).collect::<String>()
    );
}
