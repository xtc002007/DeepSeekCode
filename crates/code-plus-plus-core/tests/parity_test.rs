//! Fixture-driven behavioral parity tests aligned with specs/session-runtime.md

use code_plus_plus_config::CodePlusPlusConfig;
use code_plus_plus_core::location::empty_location_deps;
use code_plus_plus_core::session::SessionService;
use code_plus_plus_core::system_context::SystemContextRegistry;
use code_plus_plus_schema::session::PromptDelivery;
use code_plus_plus_storage::{admit_prompt, Database, EventStore, SessionStore};
use code_plus_plus_types::{MessageId, SessionId};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    given: FixtureGiven,
    when: FixtureWhen,
    then: FixtureThen,
}

#[derive(Deserialize)]
struct FixtureGiven {
    session_exists: bool,
    #[serde(default)]
    pending_inbox: usize,
}

#[derive(Deserialize)]
struct FixtureWhen {
    action: String,
    prompt: String,
    delivery: String,
    #[allow(dead_code)]
    resume: bool,
}

#[derive(Deserialize)]
struct FixtureThen {
    inbox_count: usize,
    history_count: usize,
    #[serde(default)]
    events: Vec<String>,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

async fn run_fixture(path: &PathBuf) -> Result<(), String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let fixture: Fixture = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    let dir = TempDir::new().map_err(|e| e.to_string())?;
    if fixture.name != "mandatory_context_unavailable" {
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nTest instructions.").ok();
    }

    let db = Arc::new(
        Database::open_in_memory()
            .await
            .map_err(|e| e.to_string())?,
    );
    let pool = db.pool().clone();
    let sessions = SessionStore::new(pool.clone());
    let events = Arc::new(EventStore::new(pool.clone()));

    let session_id = SessionId::new();
    if fixture.given.session_exists {
        sessions
            .create(
                session_id,
                code_plus_plus_types::LocationId::new(),
                &dir.path().to_string_lossy(),
                code_plus_plus_schema::agent::AgentKind::Build,
                None,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
    }

    if fixture.given.pending_inbox > 0 && fixture.name != "mandatory_context_unavailable" {
        for i in 0..fixture.given.pending_inbox {
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                &format!("pending-{i}"),
                PromptDelivery::Queue,
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    let delivery = if fixture.when.delivery == "steer" {
        PromptDelivery::Steer
    } else {
        PromptDelivery::Queue
    };

    match fixture.when.action.as_str() {
        "sessions.prompt" => {
            use code_plus_plus_schema::event::EventKind;
            let message_id = MessageId::new();
            admit_prompt(
                &pool,
                message_id,
                session_id,
                &fixture.when.prompt,
                delivery,
            )
            .await
            .map_err(|e| e.to_string())?;
            events
                .append(
                    EventKind::PromptAdmitted,
                    Some(session_id),
                    serde_json::json!({
                        "message_id": message_id.to_string(),
                        "delivery": fixture.when.delivery.clone(),
                    }),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.prompt_idempotent" => {
            let message_id = MessageId::new();
            for _ in 0..2 {
                admit_prompt(
                    &pool,
                    message_id,
                    session_id,
                    &fixture.when.prompt,
                    delivery,
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        "sessions.prompt_double" => {
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                &fixture.when.prompt,
                delivery,
            )
            .await
            .map_err(|e| e.to_string())?;
            admit_prompt(&pool, MessageId::new(), session_id, "second", delivery)
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.create" => {
            sessions
                .create(
                    SessionId::new(),
                    code_plus_plus_types::LocationId::new(),
                    &dir.path().to_string_lossy(),
                    code_plus_plus_schema::agent::AgentKind::Build,
                    None,
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.interrupt" => {
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                None,
            );
            service
                .interrupt(session_id)
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.move" => {
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                None,
            );
            let new_dir = dir.path().join("moved").to_string_lossy().to_string();
            std::fs::create_dir_all(&new_dir).ok();
            service
                .move_session(session_id, new_dir)
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.fork" => {
            sessions
                .insert_message(MessageId::new(), session_id, "user", &fixture.when.prompt)
                .await
                .map_err(|e| e.to_string())?;
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                None,
            );
            service
                .fork(session_id, None)
                .await
                .map_err(|e| e.to_string())?;
        }
        "sessions.active" => {
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                None,
            );
            let active = service.active().await.map_err(|e| e.to_string())?;
            assert!(
                active.is_empty(),
                "{}: expected no active sessions",
                fixture.name
            );
        }
        "context.baseline" => {
            let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
            assert!(
                registry.can_initialize_baseline(),
                "{}: expected baseline available",
                fixture.name
            );
        }
        "context.unavailable" => {
            let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
            assert!(
                !registry.can_initialize_baseline(),
                "{}: expected mandatory context unavailable",
                fixture.name
            );
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                &fixture.when.prompt,
                delivery,
            )
            .await
            .map_err(|e| e.to_string())?;
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let _service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions.clone(),
                providers.clone(),
                events.clone(),
                mcp.clone(),
                hooks.clone(),
                None,
            );
            let runner = code_plus_plus_core::session::SessionRunner::new(
                db.clone(),
                session_id,
                dir.path().to_string_lossy().to_string(),
                CodePlusPlusConfig::default(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                code_plus_plus_core::tools::services::noop_task_spawner(),
            );
            let promoted = runner
                .test_promote_at_safe_boundary()
                .await
                .map_err(|e| e.to_string())?;
            assert!(
                !promoted,
                "{}: promote should be blocked when mandatory context unavailable",
                fixture.name
            );
        }
        "context.agents_md" => {
            let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
            let baseline = registry.render_baseline().expect("baseline");
            assert!(
                baseline.baseline.contains("Agent"),
                "{}: expected AGENTS.md in baseline",
                fixture.name
            );
        }
        "context.compaction_epoch" => {
            let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
            let epochs = code_plus_plus_core::system_context::ContextEpochStore::new(pool.clone());
            registry
                .ensure_epoch(&epochs, &sessions, session_id)
                .await
                .map_err(|e| e.to_string())?;
            let epoch = epochs.get(session_id).await.map_err(|e| e.to_string())?;
            assert!(
                epoch.is_some(),
                "{}: expected context epoch after ensure",
                fixture.name
            );
        }
        "context.compaction_replaces_baseline" => {
            // Rule 5: compaction replaces the baseline from the current
            // complete context. Seed epoch v1, revise the source, rebuild
            // from the fresh complete context, and assert v2 is stored.
            let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
            let epochs = code_plus_plus_core::system_context::ContextEpochStore::new(pool.clone());
            registry
                .ensure_epoch(&epochs, &sessions, session_id)
                .await
                .map_err(|e| e.to_string())?;
            let before = epochs
                .get(session_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{}: expected epoch", fixture.name))?;
            assert!(
                before.baseline.contains("Test instructions"),
                "{}: expected v1 baseline",
                fixture.name
            );
            std::fs::write(
                dir.path().join("AGENTS.md"),
                "# Agent\nRevised instructions.",
            )
            .map_err(|e| e.to_string())?;
            let fresh = SystemContextRegistry::new(dir.path().to_string_lossy());
            let generation = fresh.render_baseline().ok_or_else(|| {
                format!(
                    "{}: expected complete context after source edit",
                    fixture.name
                )
            })?;
            assert!(
                generation.baseline.contains("Revised instructions"),
                "{}: fresh baseline must reflect the edit",
                fixture.name
            );
            let seq = sessions
                .latest_message_seq(session_id)
                .await
                .map_err(|e| e.to_string())?;
            epochs
                .replace_baseline(session_id, &generation, seq)
                .await
                .map_err(|e| e.to_string())?;
            let after = epochs
                .get(session_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{}: expected epoch", fixture.name))?;
            assert!(
                after.baseline.contains("Revised instructions"),
                "{}: stored baseline must be replaced",
                fixture.name
            );
        }
        "tool.large_output_paged" => {
            // Large outputs spill to managed files with a bounded projection;
            // `tool_result_page` must reassemble the exact original bytes.
            use code_plus_plus_core::tools::registry::Tool;
            use code_plus_plus_storage::{ManagedToolOutput, ToolOutputLimits};
            let store = ManagedToolOutput::project(dir.path());
            let big = "x".repeat(100_000);
            let bounded = store
                .store(
                    session_id,
                    "call-1",
                    "read",
                    &serde_json::json!({ "content": big }),
                    &ToolOutputLimits {
                        max_lines: 10,
                        max_bytes: 100,
                    },
                )
                .map_err(|e| e.to_string())?;
            if bounded.get("truncated").and_then(|v| v.as_bool()) != Some(true) {
                return Err(format!("{}: expected bounded projection", fixture.name));
            }
            let file = bounded
                .get("full_output_file")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{}: missing full_output_file", fixture.name))?;
            let tool = code_plus_plus_core::tools::tool_result_page::ToolResultPageTool;
            let ctx = code_plus_plus_schema::tool::ToolContext {
                session_id,
                agent: "build".into(),
                assistant_message_id: MessageId::new(),
                tool_call_id: code_plus_plus_types::ToolCallId::new(),
                working_directory: dir.path().to_string_lossy().to_string(),
                extras: Default::default(),
            };
            let mut offset = 0usize;
            let mut rebuilt = String::new();
            for _ in 0..32 {
                let out = tool
                    .execute(
                        serde_json::json!({ "path": file, "offset": offset, "limit": 8192 }),
                        &ctx,
                    )
                    .await
                    .map_err(|e| format!("{}: page failed: {}", fixture.name, e.message))?;
                rebuilt.push_str(out.get("page").and_then(|v| v.as_str()).unwrap_or(""));
                offset = out.get("next_offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if out.get("complete").and_then(|v| v.as_bool()) == Some(true) {
                    break;
                }
            }
            // The managed file holds the full envelope JSON (not the bare
            // content string), so pages must reassemble to the FILE bytes.
            let file_bytes = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            if rebuilt != file_bytes {
                return Err(format!(
                    "{}: paged reassembly mismatch ({} vs {} bytes)",
                    fixture.name,
                    rebuilt.len(),
                    file_bytes.len()
                ));
            }
            // And the envelope still parses back to the original content.
            let parsed: serde_json::Value =
                serde_json::from_str(&file_bytes).map_err(|e| e.to_string())?;
            if parsed.get("content").and_then(|v| v.as_str()) != Some(big.as_str()) {
                return Err(format!("{}: envelope content mismatch", fixture.name));
            }
        }
        "permission.ask" => {
            use code_plus_plus_config::PolicyScopeConfig;
            use code_plus_plus_schema::agent::AgentKind;
            use code_plus_plus_schema::policy::{PolicyAction, PolicyReply, PolicyReplyKind};
            let mut config = code_plus_plus_config::CodePlusPlusConfig::default();
            config.policy.insert(
                "bash".into(),
                PolicyScopeConfig::Simple(PolicyAction::Prompt),
            );
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::new(
                &config,
                dir.path().to_string_lossy(),
                None,
            ));
            let permissions_wait = permissions.clone();
            let handle = tokio::spawn(async move {
                let _ = permissions_wait
                    .ensure_allowed(
                        session_id,
                        "bash",
                        &AgentKind::Build,
                        serde_json::json!({ "command": "echo hi" }),
                    )
                    .await;
            });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let pending = permissions.list_pending().await;
            if pending.len() != 1 {
                return Err(format!(
                    "{}: expected 1 pending permission, got {}",
                    fixture.name,
                    pending.len()
                ));
            }
            permissions
                .reply(
                    pending[0].id,
                    PolicyReply {
                        reply: PolicyReplyKind::Reject,
                        message: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            let _ = handle.await;
        }
        "permission.plan_deny" => {
            use code_plus_plus_schema::agent::AgentKind;
            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let err = permissions
                .ensure_allowed(
                    session_id,
                    "bash",
                    &AgentKind::Plan,
                    serde_json::json!({ "command": "echo hi" }),
                )
                .await
                .err();
            if err.is_none() {
                return Err(format!(
                    "{}: expected plan agent to block bash",
                    fixture.name
                ));
            }
            let pending = permissions.list_pending().await;
            if !pending.is_empty() {
                return Err(format!(
                    "{}: expected no pending permission for blocked bash, got {}",
                    fixture.name,
                    pending.len()
                ));
            }
        }
        "runner.steer_mid_turn" => {
            use code_plus_plus_core::session::SessionRunner;
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                "queued while turn active",
                PromptDelivery::Queue,
            )
            .await
            .map_err(|e| e.to_string())?;
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                &fixture.when.prompt,
                PromptDelivery::Steer,
            )
            .await
            .map_err(|e| e.to_string())?;

            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let runner = SessionRunner::new(
                db.clone(),
                session_id,
                dir.path().to_string_lossy().to_string(),
                CodePlusPlusConfig::default(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                code_plus_plus_core::tools::services::noop_task_spawner(),
            );
            runner.test_set_in_provider_turn(true);
            let promoted = runner
                .test_promote_at_safe_boundary()
                .await
                .map_err(|e| e.to_string())?;
            if !promoted {
                return Err(format!(
                    "{}: expected steer promotion during provider turn",
                    fixture.name
                ));
            }
        }
        "runner.drain_queue" => {
            use code_plus_plus_core::session::SessionRunner;
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                &fixture.when.prompt,
                PromptDelivery::Queue,
            )
            .await
            .map_err(|e| e.to_string())?;
            admit_prompt(
                &pool,
                MessageId::new(),
                session_id,
                "second prompt",
                PromptDelivery::Queue,
            )
            .await
            .map_err(|e| e.to_string())?;

            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let runner = SessionRunner::new(
                db.clone(),
                session_id,
                dir.path().to_string_lossy().to_string(),
                CodePlusPlusConfig::default(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                code_plus_plus_core::tools::services::noop_task_spawner(),
            );
            // Idle safe boundary promotes ALL pending in admission order
            // (single call drains both). Second call must be no-op.
            let first = runner
                .test_promote_at_safe_boundary()
                .await
                .map_err(|e| e.to_string())?;
            if !first {
                return Err(format!(
                    "{}: expected queue promotion at safe boundary",
                    fixture.name
                ));
            }
            let second = runner
                .test_promote_at_safe_boundary()
                .await
                .map_err(|e| e.to_string())?;
            if second {
                return Err(format!(
                    "{}: second promotion should be no-op after full drain",
                    fixture.name
                ));
            }
        }
        "sessions.revert_stage" => {
            let user_id = MessageId::new();
            sessions
                .insert_message(user_id, session_id, "user", &fixture.when.prompt)
                .await
                .map_err(|e| e.to_string())?;

            let demo_file = dir.path().join("demo.txt");
            std::fs::write(&demo_file, "original").map_err(|e| e.to_string())?;
            let tool_msg = MessageId::new();
            let snapshots =
                code_plus_plus_core::snapshot::FileSnapshotStore::new(pool.clone(), dir.path(), true);
            snapshots
                .capture(session_id, Some(tool_msg), &demo_file)
                .await
                .map_err(|e| e.to_string())?;
            std::fs::write(&demo_file, "mutated").map_err(|e| e.to_string())?;
            sessions
                .insert_tool_result(
                    tool_msg,
                    session_id,
                    0,
                    "call-1",
                    "write",
                    &serde_json::json!({ "path": "demo.txt" }),
                )
                .await
                .map_err(|e| e.to_string())?;

            let permissions = Arc::new(code_plus_plus_core::permission::PermissionService::default());
            let providers = Arc::new(code_plus_plus_core::provider::ProviderCatalog::new());
            let (mcp, hooks) = empty_location_deps();
            let service = SessionService::new(
                db.clone(),
                CodePlusPlusConfig::default(),
                dir.path().to_string_lossy().to_string(),
                permissions,
                providers,
                events.clone(),
                mcp,
                hooks,
                None,
            );
            let staged = service
                .revert_stage(session_id, user_id)
                .await
                .map_err(|e| e.to_string())?;
            assert_eq!(
                staged.staged_message_id, user_id,
                "{}: staged message mismatch",
                fixture.name
            );
            assert_eq!(
                std::fs::read_to_string(&demo_file).map_err(|e| e.to_string())?,
                "original",
                "{}: file not restored on stage",
                fixture.name
            );
            assert!(
                !staged.files.is_empty(),
                "{}: expected staged file diff entries",
                fixture.name
            );

            service
                .revert_commit(session_id)
                .await
                .map_err(|e| e.to_string())?;
            use code_plus_plus_core::session::projector::EventProjector;
            let messages = EventProjector::project_messages(&sessions, &events, session_id)
                .await
                .map_err(|e| e.to_string())?;
            if messages.len() != 1 {
                return Err(format!(
                    "{}: expected history 1 after commit, got {}",
                    fixture.name,
                    messages.len()
                ));
            }
        }
        "overflow.detect" => {
            use code_plus_plus_llm::is_context_overflow;
            let message = "prompt is too long: 200001 tokens > 200000 maximum";
            assert!(
                is_context_overflow(message),
                "{}: expected overflow detection",
                fixture.name
            );
        }
        "mcp.tool_bridge" => {
            use code_plus_plus_core::mcp::bridged_tool_name;
            assert_eq!(
                bridged_tool_name("myserver", "read"),
                "mcp_myserver_read",
                "{}: bridged tool name mismatch",
                fixture.name
            );
        }
        "todo.update" => {
            use code_plus_plus_schema::event::EventKind;
            use code_plus_plus_schema::todo::TodoItem;
            use code_plus_plus_storage::TodoStore;
            let store = TodoStore::new(pool.clone());
            let items = vec![TodoItem {
                content: "Ship feature".into(),
                status: "pending".into(),
                priority: "high".into(),
            }];
            let saved = store
                .update(session_id, &items)
                .await
                .map_err(|e| e.to_string())?;
            events
                .append(
                    EventKind::TodoUpdated,
                    Some(session_id),
                    serde_json::json!({ "todos": saved }),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        "activity.project" => {
            use chrono::Utc;
            use code_plus_plus_core::activity::ActivityProjector;
            use code_plus_plus_schema::activity::ActivityGroupKind;
            use code_plus_plus_schema::event::{EventEnvelope, EventKind};
            let evs = vec![
                EventEnvelope {
                    seq: 1,
                    kind: EventKind::ProviderTurnStarted,
                    session_id: Some(session_id),
                    payload: serde_json::json!({}),
                    created_at: Utc::now(),
                },
                EventEnvelope {
                    seq: 2,
                    kind: EventKind::ToolCompleted,
                    session_id: Some(session_id),
                    payload: serde_json::json!({
                        "tool_call_id": "e1",
                        "tool_name": "edit",
                        "metadata": { "path": "a.rs", "diff_stats": { "added": 21, "removed": 1 } },
                        "duration_ms": 5
                    }),
                    created_at: Utc::now(),
                },
                EventEnvelope {
                    seq: 3,
                    kind: EventKind::ToolCompleted,
                    session_id: Some(session_id),
                    payload: serde_json::json!({
                        "tool_call_id": "g1",
                        "tool_name": "grep",
                        "metadata": { "match_count": 3 },
                        "duration_ms": 2
                    }),
                    created_at: Utc::now(),
                },
                EventEnvelope {
                    seq: 4,
                    kind: EventKind::SessionIdle,
                    session_id: Some(session_id),
                    payload: serde_json::json!({}),
                    created_at: Utc::now(),
                },
            ];
            let tree = ActivityProjector::project(&evs, &[]);
            for ev in &evs {
                events
                    .append(ev.kind, ev.session_id, ev.payload.clone())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let group = tree
                .roots
                .iter()
                .find_map(|n| match n {
                    code_plus_plus_schema::activity::ActivityNode::Group(g)
                        if g.kind == ActivityGroupKind::ProviderTurn =>
                    {
                        Some(g)
                    }
                    _ => None,
                })
                .expect("provider turn group");
            let summary = group.summary.as_ref().expect("summary");
            assert_eq!(summary.files_edited, 1, "{}: edit count", fixture.name);
            assert_eq!(summary.searches, 1, "{}: search count", fixture.name);
            assert_eq!(summary.lines_added, 21, "{}: lines added", fixture.name);
            assert_eq!(summary.lines_removed, 1, "{}: lines removed", fixture.name);
        }
        "tool.completed" => {
            use code_plus_plus_core::tools::display::compute_text_diff_stats;
            use code_plus_plus_schema::event::EventKind;
            use code_plus_plus_schema::tool::ToolResultEnvelope;
            let stats = compute_text_diff_stats("line\n", "line\nadded\n");
            assert!(
                stats.added >= 1,
                "{}: expected positive diff stats",
                fixture.name
            );
            let envelope =
                ToolResultEnvelope::new("Edited foo.rs", serde_json::json!({ "content": "x" }))
                    .with_metadata(serde_json::json!({
                        "path": "foo.rs",
                        "diff_stats": { "added": 2, "removed": 0 }
                    }))
                    .with_duration_ms(5);
            let stored = envelope.to_stored();
            let parsed = ToolResultEnvelope::from_stored(&stored);
            assert_eq!(parsed.title, "Edited foo.rs");
            assert_eq!(parsed.duration_ms, 5);
            assert_eq!(parsed.metadata["diff_stats"]["added"], 2);
            events
                .append(
                    EventKind::ToolCompleted,
                    Some(session_id),
                    serde_json::json!({
                        "tool_call_id": "e1",
                        "tool_name": "edit",
                        "title": parsed.title,
                        "metadata": parsed.metadata,
                        "output": parsed.output,
                        "duration_ms": parsed.duration_ms
                    }),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        "skill.discover" => {
            use code_plus_plus_core::skills::SkillCatalog;
            let skills_dir = dir
                .path()
                .join(".code-plus-plus")
                .join("skills")
                .join("demo-skill");
            std::fs::create_dir_all(&skills_dir).map_err(|e| e.to_string())?;
            std::fs::write(
                skills_dir.join("SKILL.md"),
                "---\nname: demo-skill\ndescription: Demo skill\n---\n# Demo",
            )
            .map_err(|e| e.to_string())?;
            let skills = SkillCatalog::discover(&[], &dir.path().to_string_lossy(), None);
            assert!(
                skills.iter().any(|s| s.name == "demo-skill"),
                "{}: expected demo-skill discovered",
                fixture.name
            );
        }
        other => return Err(format!("{}: unknown action {other}", fixture.name)),
    }

    // P0-7: parenthesized (`&&` binds tighter than `||`); previously
    // `session_exists=false` prompt fixtures entered the branch while
    // `session_exists=true,inbox=0` cases were skipped.
    if (fixture.given.session_exists && fixture.then.inbox_count > 0)
        || (fixture.when.action.starts_with("sessions.prompt") && fixture.given.session_exists)
    {
        let pending = sessions
            .pending_inputs(session_id)
            .await
            .map_err(|e| e.to_string())?;
        if pending.len() != fixture.then.inbox_count {
            return Err(format!(
                "{}: expected inbox {}, got {}",
                fixture.name,
                fixture.then.inbox_count,
                pending.len()
            ));
        }
    }

    if fixture.then.history_count > 0 {
        use code_plus_plus_core::session::projector::EventProjector;
        let messages = EventProjector::project_messages(&sessions, &events, session_id)
            .await
            .map_err(|e| e.to_string())?;
        if messages.len() != fixture.then.history_count {
            return Err(format!(
                "{}: expected history {}, got {}",
                fixture.name,
                fixture.then.history_count,
                messages.len()
            ));
        }
    }

    if !fixture.then.events.is_empty() {
        let evs = events
            .tail_filtered(0, 1000, Some(session_id))
            .await
            .map_err(|e| e.to_string())?;
        let kinds: Vec<String> = evs.iter().map(|e| e.kind.as_str().to_string()).collect();
        for expected in &fixture.then.events {
            if !kinds.iter().any(|k| k == expected) {
                return Err(format!(
                    "{}: expected event `{expected}` in {:?}",
                    fixture.name, kinds
                ));
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn run_all_fixtures() {
    let dir = fixtures_dir();
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            run_fixture(&path)
                .await
                .unwrap_or_else(|e| panic!("fixture {path:?}: {e}"));
            count += 1;
        }
    }
    assert!(count >= 27, "expected at least 27 fixtures, got {count}");
}

#[tokio::test]
async fn prompt_admission_fixture() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agent").unwrap();

    let db = Database::open_in_memory().await.unwrap();
    let pool = db.pool().clone();
    let sessions = SessionStore::new(pool.clone());

    let session_id = SessionId::new();
    sessions
        .create(
            session_id,
            code_plus_plus_types::LocationId::new(),
            &dir.path().to_string_lossy(),
            code_plus_plus_schema::agent::AgentKind::Build,
            None,
            None,
        )
        .await
        .unwrap();

    let message_id = MessageId::new();
    admit_prompt(
        &pool,
        message_id,
        session_id,
        "hello",
        PromptDelivery::Queue,
    )
    .await
    .unwrap();

    let pending = sessions.pending_inputs(session_id).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].prompt, "hello");
    assert!(!pending[0].promoted);
}
