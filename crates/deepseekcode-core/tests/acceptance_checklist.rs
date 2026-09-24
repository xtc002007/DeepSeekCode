//! Automated coverage for ui-update-plan.md section 6.2 (core/runtime items).

use chrono::Utc;
use deepseekcode_core::activity::ActivityProjector;
use deepseekcode_core::tools::display::{build_tool_result, enrich_metadata};
use deepseekcode_schema::activity::{
    ActivityGroupKind, ActivityKind, ActivityNode, ActivityTargetKind,
};
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::todo::TodoItem;
use deepseekcode_types::{MessageId, SessionId, ToolCallId};
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
fn acceptance_read_metadata_uses_actual_line_range() {
    let meta = enrich_metadata(
        "read",
        &json!({ "path": "app/page.js", "offset": 1, "limit": 2000 }),
        &json!({
            "path": "app/page.js",
            "content": "line1\nline2\nline3",
            "line_start": 1,
            "line_end": 3
        }),
    );
    assert_eq!(meta["line_start"], 1);
    assert_eq!(meta["line_end"], 3);
}

#[test]
fn acceptance_edit_diff_stats_in_turn_summary() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "e1",
                "tool_name": "edit",
                "metadata": { "path": "foo.rs", "diff_stats": { "added": 21, "removed": 1 } },
                "duration_ms": 3
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let tree = ActivityProjector::project(&events, &[]);
    let summary = tree
        .roots
        .iter()
        .find_map(|n| match n {
            ActivityNode::Group(g) if g.kind == ActivityGroupKind::ProviderTurn => {
                g.summary.as_ref()
            }
            _ => None,
        })
        .expect("summary");
    assert_eq!(summary.lines_added, 21);
    assert_eq!(summary.lines_removed, 1);
}

#[test]
fn acceptance_bash_exit_code_metadata() {
    let envelope = build_tool_result(
        "bash",
        &json!({ "command": "npm test" }),
        json!({ "stdout": "ok", "stderr": "", "exit_code": 0 }),
        12,
        None,
        json!({}),
    );
    assert_eq!(envelope.metadata["exit_code"], 0);
}

#[test]
fn acceptance_todo_leaf_focus_index() {
    let events = vec![ev(
        EventKind::TodoUpdated,
        json!({
            "todos": [
                { "content": "a", "status": "pending", "priority": "medium" },
                { "content": "b", "status": "in_progress", "priority": "high" }
            ]
        }),
    )];
    let tree = ActivityProjector::project(&events, &[]);
    let leaf = tree.roots.iter().find_map(|n| match n {
        ActivityNode::Leaf(l) if l.kind == ActivityKind::TodoWrite => Some(l),
        _ => None,
    });
    assert_eq!(leaf.expect("leaf").meta.todo_focus_index, Some(1));
}

#[test]
fn acceptance_task_session_target() {
    let events = vec![ev(
        EventKind::ToolCompleted,
        json!({
            "tool_call_id": "t1",
            "tool_name": "task",
            "output": {
                "title": "Task",
                "output": { "session_id": "child-session", "agent": "build" },
                "metadata": { "session_id": "child-session" },
                "duration_ms": 1
            }
        }),
    )];
    let tree = ActivityProjector::project(&events, &[]);
    let session_target = tree.roots.iter().find_map(|n| match n {
        ActivityNode::Group(g) if g.kind == ActivityGroupKind::TaskSubagent => {
            g.children.iter().find_map(|c| match c {
                ActivityNode::Leaf(l) => Some(&l.target),
                _ => None,
            })
        }
        _ => None,
    });
    assert!(matches!(
        session_target,
        Some(ActivityTargetKind::Session { ref session_id, .. }) if session_id == "child-session"
    ));
}

#[tokio::test]
async fn acceptance_question_answer_roundtrip() {
    use deepseekcode_core::tools::question::QuestionTool;
    use deepseekcode_core::tools::registry::Tool;
    use deepseekcode_core::tools::services::{submit_question_answer, ToolServices};
    use deepseekcode_types::{SessionId, ToolCallId};
    use std::sync::Arc;

    let question_tx = Arc::new(tokio::sync::Mutex::new(None));
    let services = ToolServices {
        question_tx: Some(question_tx.clone()),
        ..Default::default()
    };
    let ctx = deepseekcode_schema::tool::ToolContext {
        session_id: SessionId::new(),
        tool_call_id: ToolCallId::new(),
        agent: "build".into(),
        assistant_message_id: deepseekcode_types::MessageId::new(),
        working_directory: ".".into(),
        extras: Default::default(),
    };
    let answer_task = tokio::spawn(async move {
        deepseekcode_core::tools::services::with_services_async(services.clone(), async {
            QuestionTool
                .execute(json!({ "question": "Continue?", "options": ["yes"] }), &ctx)
                .await
                .expect("question")
        })
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    submit_question_answer(
        &ToolServices {
            question_tx: Some(question_tx),
            ..Default::default()
        },
        "yes".into(),
    )
    .await
    .expect("submit");
    assert_eq!(answer_task.await.expect("join")["answer"], "yes");
}

#[test]
fn acceptance_hydrate_projector_idempotent() {
    let events = vec![
        ev(EventKind::ProviderTurnStarted, json!({})),
        ev(
            EventKind::ToolCompleted,
            json!({
                "tool_call_id": "r1",
                "tool_name": "read",
                "metadata": { "path": "a.rs", "line_start": 1, "line_end": 5 },
                "duration_ms": 1
            }),
        ),
        ev(EventKind::SessionIdle, json!({})),
    ];
    let a = ActivityProjector::project(&events, &[]);
    let b = ActivityProjector::project(&events, &[]);
    assert_eq!(a.roots.len(), b.roots.len());
}

#[tokio::test]
async fn acceptance_todo_store_roundtrip() {
    use deepseekcode_storage::{Database, TodoStore};
    use deepseekcode_types::SessionId;
    use std::sync::Arc;

    let db = Arc::new(Database::open_in_memory().await.expect("db"));
    let store = TodoStore::new(db.pool().clone());
    let session_id = SessionId::new();
    let items = vec![TodoItem {
        content: "Ship".into(),
        status: "in_progress".into(),
        priority: "high".into(),
    }];
    let saved = store.update(session_id, &items).await.expect("update");
    let loaded = store.get(session_id).await.expect("get");
    assert_eq!(saved, loaded);
}

#[tokio::test]
async fn acceptance_list_tool_lists_directory_entries() {
    use deepseekcode_core::tools::list::ListTool;
    use deepseekcode_core::tools::registry::Tool;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.txt"), "a").expect("write");
    let ctx = deepseekcode_schema::tool::ToolContext {
        session_id: SessionId::new(),
        tool_call_id: ToolCallId::new(),
        agent: "build".into(),
        assistant_message_id: MessageId::new(),
        working_directory: dir.path().to_string_lossy().to_string(),
        extras: Default::default(),
    };
    let out = ListTool
        .execute(serde_json::json!({ "path": "." }), &ctx)
        .await
        .expect("list execute");
    let entries = out["entries"].as_array().expect("entries");
    assert!(entries.iter().any(|e| e["name"] == "alpha.txt"));
}
