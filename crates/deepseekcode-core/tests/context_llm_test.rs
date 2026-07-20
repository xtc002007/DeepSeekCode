//! Verifies post-baseline system messages become LLM user updates.

use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_core::location::empty_location_deps;
use deepseekcode_core::session::runner::SessionRunner;
use deepseekcode_core::system_context::{ContextEpochStore, SystemContextRegistry};
use deepseekcode_core::tools::services::noop_task_spawner;
use deepseekcode_storage::{Database, EventStore, SessionStore};
use deepseekcode_types::{MessageId, SessionId};
use std::sync::Arc;

#[tokio::test]
async fn build_llm_messages_maps_system_updates_to_user_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nDo things.").unwrap();

    let db = Arc::new(Database::open_in_memory().await.expect("db"));
    let pool = db.pool().clone();
    let sessions = SessionStore::new(pool.clone());
    let events = Arc::new(EventStore::new(pool.clone()));
    let project_dir = dir.path().to_string_lossy().to_string();
    let session_id = SessionId::new();
    sessions
        .create(
            session_id,
            deepseekcode_types::LocationId::new(),
            &project_dir,
            deepseekcode_schema::agent::AgentKind::Build,
            None,
            None,
        )
        .await
        .expect("create session");

    sessions
        .insert_message(MessageId::new(), session_id, "user", "hello")
        .await
        .expect("insert user");

    let registry = SystemContextRegistry::new(&project_dir);
    let epochs = ContextEpochStore::new(pool.clone());
    let baseline_seq = events.latest_seq(session_id).await.expect("latest seq");
    if let Some(gen) = registry.render_baseline() {
        epochs
            .initialize(session_id, &gen, baseline_seq)
            .await
            .expect("init epoch");
    }

    sessions
        .insert_message(MessageId::new(), session_id, "system", "tools changed")
        .await
        .expect("insert system update");

    let (mcp, hooks) = empty_location_deps();
    let runner = SessionRunner::new(
        db,
        session_id,
        project_dir.clone(),
        DeepSeekCodeConfig::default(),
        Arc::new(deepseekcode_core::permission::PermissionService::new(
            &DeepSeekCodeConfig::default(),
            &project_dir,
            None,
        )),
        Arc::new(deepseekcode_core::provider::ProviderCatalog::new()),
        events,
        mcp,
        hooks,
        noop_task_spawner(),
    );

    let messages = runner
        .test_build_llm_messages()
        .await
        .expect("build messages");
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("[System update]: tools changed")),
        "expected system update mapped into llm messages: {:?}",
        messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn build_llm_messages_excludes_pre_baseline_system_messages() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nBaseline.").unwrap();

    let db = Arc::new(Database::open_in_memory().await.expect("db"));
    let pool = db.pool().clone();
    let sessions = SessionStore::new(pool.clone());
    let events = Arc::new(EventStore::new(pool.clone()));
    let project_dir = dir.path().to_string_lossy().to_string();
    let session_id = SessionId::new();
    sessions
        .create(
            session_id,
            deepseekcode_types::LocationId::new(),
            &project_dir,
            deepseekcode_schema::agent::AgentKind::Build,
            None,
            None,
        )
        .await
        .expect("create session");

    sessions
        .insert_message(MessageId::new(), session_id, "system", "old baseline note")
        .await
        .expect("insert old system");
    sessions
        .insert_message(MessageId::new(), session_id, "user", "hello")
        .await
        .expect("insert user");

    let registry = SystemContextRegistry::new(&project_dir);
    let epochs = ContextEpochStore::new(pool.clone());
    let baseline_seq = sessions
        .list_messages(session_id)
        .await
        .expect("list")
        .last()
        .map(|m| m.seq)
        .unwrap_or(0);
    if let Some(gen) = registry.render_baseline() {
        epochs
            .initialize(session_id, &gen, baseline_seq)
            .await
            .expect("init epoch");
    }

    let (mcp, hooks) = empty_location_deps();
    let runner = SessionRunner::new(
        db,
        session_id,
        project_dir.clone(),
        DeepSeekCodeConfig::default(),
        Arc::new(deepseekcode_core::permission::PermissionService::new(
            &DeepSeekCodeConfig::default(),
            &project_dir,
            None,
        )),
        Arc::new(deepseekcode_core::provider::ProviderCatalog::new()),
        events,
        mcp,
        hooks,
        noop_task_spawner(),
    );

    let messages = runner
        .test_build_llm_messages()
        .await
        .expect("build messages");
    assert!(
        !messages
            .iter()
            .any(|m| m.content.contains("old baseline note")),
        "pre-baseline system messages must not appear in llm context"
    );
}
