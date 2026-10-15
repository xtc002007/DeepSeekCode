use code_plus_plus_schema::agent::AgentKind;
use code_plus_plus_storage::{Database, SessionStore};
use code_plus_plus_types::{MessageId, SessionId};
use std::sync::Arc;

async fn open_store() -> (SessionStore, SessionId) {
    let db = Arc::new(Database::open_in_memory().await.expect("db"));
    let store = SessionStore::new(db.pool().clone());
    let session_id = SessionId::new();
    store
        .create(
            session_id,
            code_plus_plus_types::LocationId::new(),
            "/tmp/test",
            AgentKind::Build,
            None,
            None,
        )
        .await
        .expect("create session");
    (store, session_id)
}

#[tokio::test]
async fn message_seq_increments_monotonically() {
    let (store, session_id) = open_store().await;
    store
        .insert_message(MessageId::new(), session_id, "user", "a")
        .await
        .expect("insert a");
    store
        .insert_message(MessageId::new(), session_id, "user", "b")
        .await
        .expect("insert b");
    let messages = store.list_messages(session_id).await.expect("list");
    assert_eq!(messages.len(), 2);
    assert!(messages[0].seq < messages[1].seq);
}

#[tokio::test]
async fn assistant_turn_and_tool_result_share_transactional_seq() {
    let (store, session_id) = open_store().await;
    let assistant_id = MessageId::new();
    store
        .insert_assistant_turn(assistant_id, session_id, None, "hi")
        .await
        .expect("assistant");
    store
        .append_tool_call(
            assistant_id,
            0,
            "call-1",
            "read",
            &serde_json::json!({ "path": "README.md" }),
        )
        .await
        .expect("tool call part");
    store
        .insert_tool_result(
            MessageId::new(),
            session_id,
            0,
            "call-1",
            "read",
            &serde_json::json!({ "success": true }),
        )
        .await
        .expect("tool result");
    let messages = store.list_messages(session_id).await.expect("list");
    assert_eq!(messages.len(), 2);
    let assistant = messages
        .iter()
        .find(|m| m.id == assistant_id)
        .expect("assistant");
    assert_eq!(assistant.parts.len(), 2);
}

#[tokio::test]
async fn concurrent_inserts_get_unique_seq() {
    let (store, session_id) = open_store().await;
    let store = Arc::new(store);
    let mut handles = Vec::new();
    for i in 0..8 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            store
                .insert_message(MessageId::new(), session_id, "user", &format!("msg-{i}"))
                .await
        }));
    }
    for handle in handles {
        handle.await.expect("join").expect("insert");
    }
    let messages = store.list_messages(session_id).await.expect("list");
    assert_eq!(messages.len(), 8);
    let seqs: Vec<i64> = messages.iter().map(|m| m.seq).collect();
    let mut unique = seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), seqs.len(), "duplicate seq detected: {seqs:?}");
}
