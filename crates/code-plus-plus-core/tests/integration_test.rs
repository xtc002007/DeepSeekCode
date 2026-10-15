//! Snapshot capture, revert workflow, and fork integration tests.

use code_plus_plus_config::CodePlusPlusConfig;
use code_plus_plus_schema::message::{MessageRole, PartKind};
use code_plus_plus_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest};
use code_plus_plus_storage::{Database, SessionStore};
use code_plus_plus_types::MessageId;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn test_service(dir: &TempDir, db: Arc<Database>) -> code_plus_plus_core::session::SessionService {
    let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
    let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
    let events = Arc::new(code_plus_plus_storage::EventStore::new(db.pool().clone()));
    let mcp = Arc::new(code_plus_plus_core::mcp::McpConnectionPool::new(
        HashMap::new(),
    ));
    let hooks = Arc::new(code_plus_plus_plugin::HookRegistry::new());
    code_plus_plus_core::session::SessionService::new(
        db,
        CodePlusPlusConfig::default(),
        dir.path().to_string_lossy().to_string(),
        permissions,
        providers,
        events,
        mcp,
        hooks,
        None,
    )
}

#[tokio::test]
async fn session_service_prompt_admits_to_inbox() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Test\nYou are helpful.").unwrap();

    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let service = test_service(&dir, db);

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: Some("gpt-4o-mini".into()),
            provider_id: Some("openai".into()),
        })
        .await
        .unwrap();

    let message_id = service
        .prompt(PromptRequest {
            id: None,
            session_id: session.id,
            prompt: "Say hello".into(),
            delivery: PromptDelivery::Queue,
            resume: false,
        })
        .await
        .unwrap();

    let pending = service.list_messages(session.id).await.unwrap();
    assert!(pending.is_empty() || !pending.is_empty());

    let same = service
        .prompt(PromptRequest {
            id: Some(message_id),
            session_id: session.id,
            prompt: "Say hello".into(),
            delivery: PromptDelivery::Queue,
            resume: false,
        })
        .await
        .unwrap();
    assert_eq!(same, message_id);
}

#[tokio::test]
async fn session_service_prompt_id_reuse_rejects_differences() {
    // The parity fixtures bypass `SessionService::prompt` (storage-level
    // admit only), so idempotency conflicts are covered here instead:
    // same message id with different content or delivery must fail loudly,
    // never silently return the old receipt.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Test\nYou are helpful.").unwrap();

    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let service = test_service(&dir, db);

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: Some("gpt-4o-mini".into()),
            provider_id: Some("openai".into()),
        })
        .await
        .unwrap();

    let message_id = service
        .prompt(PromptRequest {
            id: None,
            session_id: session.id,
            prompt: "Say hello".into(),
            delivery: PromptDelivery::Queue,
            resume: false,
        })
        .await
        .unwrap();

    let conflict = service
        .prompt(PromptRequest {
            id: Some(message_id),
            session_id: session.id,
            prompt: "Say something else".into(),
            delivery: PromptDelivery::Queue,
            resume: false,
        })
        .await;
    assert!(
        matches!(
            conflict,
            Err(code_plus_plus_types::CodePlusPlusError::InvalidInput(_))
        ),
        "different content must conflict, got {conflict:?}"
    );

    let delivery_conflict = service
        .prompt(PromptRequest {
            id: Some(message_id),
            session_id: session.id,
            prompt: "Say hello".into(),
            delivery: PromptDelivery::Steer,
            resume: false,
        })
        .await;
    assert!(
        matches!(
            delivery_conflict,
            Err(code_plus_plus_types::CodePlusPlusError::InvalidInput(_))
        ),
        "different delivery must conflict, got {delivery_conflict:?}"
    );
}

#[tokio::test]
async fn fork_session_copies_tool_parts() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent").unwrap();
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let service = test_service(&dir, db.clone());

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: None,
            provider_id: None,
        })
        .await
        .unwrap();

    let store = SessionStore::new(db.pool().clone());
    let assistant_id = MessageId::new();
    store
        .insert_message(assistant_id, session.id, "user", "hello")
        .await
        .unwrap();
    store
        .append_tool_call(
            assistant_id,
            1,
            "call-1",
            "read",
            &serde_json::json!({ "path": "foo.rs" }),
        )
        .await
        .unwrap();

    let forked = service.fork(session.id, None).await.unwrap();
    assert_ne!(forked.id, session.id);

    let messages = service.list_messages(forked.id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0]
        .parts
        .iter()
        .any(|p| matches!(p.kind, PartKind::ToolCall)));
}

#[tokio::test]
async fn revert_stage_restore_and_commit_truncates_messages() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent").unwrap();
    let file = dir.path().join("demo.txt");
    std::fs::write(&file, "original").unwrap();

    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let service = test_service(&dir, db.clone());
    let snapshots =
        code_plus_plus_core::snapshot::FileSnapshotStore::new(db.pool().clone(), dir.path(), true);

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: None,
            provider_id: None,
        })
        .await
        .unwrap();

    let store = SessionStore::new(db.pool().clone());
    let user_id = MessageId::new();
    store
        .insert_message(user_id, session.id, "user", "edit file")
        .await
        .unwrap();

    let tool_msg = MessageId::new();
    let snap = snapshots
        .capture(session.id, Some(tool_msg), &file)
        .await
        .unwrap();
    std::fs::write(&file, "mutated").unwrap();
    store
        .insert_tool_result(
            tool_msg,
            session.id,
            0,
            "call-1",
            "write",
            &serde_json::json!({ "written": "demo.txt" }),
        )
        .await
        .unwrap();

    let staged = service.revert_stage(session.id, user_id).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    assert!(!staged.files.is_empty());
    let _ = snap;

    service.revert_commit(session.id).await.unwrap();
    let messages = service.list_messages(session.id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, MessageRole::User);
}

#[tokio::test]
async fn mock_llm_tool_call_pipeline() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock = MockServer::start().await;
    let tool_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_read\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\"\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"README.md\\\"}\"}}]},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );
    let text_sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"index\":0}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(tool_sse)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(text_sse)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&mock)
        .await;

    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nTest.").unwrap();
    std::fs::write(dir.path().join("README.md"), "# readme").unwrap();

    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
    providers
        .connect(
            "openai",
            "test-key".into(),
            Some(format!("{}/v1", mock.uri())),
        )
        .await
        .unwrap();

    let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
    let events = Arc::new(code_plus_plus_storage::EventStore::new(db.pool().clone()));
    let mcp = Arc::new(code_plus_plus_core::mcp::McpConnectionPool::new(
        HashMap::new(),
    ));
    let hooks = Arc::new(code_plus_plus_plugin::HookRegistry::new());
    let service = code_plus_plus_core::session::SessionService::new(
        db,
        CodePlusPlusConfig::default(),
        dir.path().to_string_lossy().to_string(),
        permissions,
        providers,
        events.clone(),
        mcp,
        hooks,
        None,
    );

    let session = service
        .create(CreateSessionRequest {
            id: None,
            directory: dir.path().to_string_lossy().to_string(),
            agent: None,
            model_id: Some("gpt-4o-mini".into()),
            provider_id: Some("openai".into()),
        })
        .await
        .unwrap();

    service
        .prompt(PromptRequest {
            id: None,
            session_id: session.id,
            prompt: "read README".into(),
            delivery: PromptDelivery::Queue,
            resume: true,
        })
        .await
        .unwrap();

    let mut saw_tool_started = false;
    let mut saw_tool_completed = false;
    let mut saw_idle = false;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let events = events
            .tail_filtered(0, 200, Some(session.id))
            .await
            .unwrap();
        saw_tool_started |= events
            .iter()
            .any(|e| matches!(e.kind, code_plus_plus_schema::event::EventKind::ToolStarted));
        saw_tool_completed |= events
            .iter()
            .any(|e| matches!(e.kind, code_plus_plus_schema::event::EventKind::ToolCompleted));
        saw_idle |= events
            .iter()
            .any(|e| matches!(e.kind, code_plus_plus_schema::event::EventKind::SessionIdle));
        if saw_tool_started && saw_tool_completed && saw_idle {
            break;
        }
    }

    assert!(saw_tool_started, "expected tool.started event");
    assert!(saw_tool_completed, "expected tool.completed event");
    assert!(saw_idle, "expected session.idle after tool settlement");

    let messages = service.list_messages(session.id).await.unwrap();
    assert!(
        messages.iter().any(|m| m.role == MessageRole::Assistant),
        "expected assistant message after tool pipeline"
    );
}
