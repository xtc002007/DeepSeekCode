//! Plan / build workflow policy and tool tests.

use deepseekcode_config::{DeepSeekCodeConfig, PolicyMode};
use deepseekcode_core::policy::service::{PolicyOutcome, PolicyService};
use deepseekcode_core::tools::plan_complete::PlanCompleteTool;
use deepseekcode_core::tools::registry::Tool;
use deepseekcode_core::tools::services::{with_services_async, ToolServices};
use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::tool::ToolContext;
use deepseekcode_storage::{Database, EventStore};
use deepseekcode_types::{MessageId, SessionId, ToolCallId};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn plan_agent_blocks_bash() {
    let permissions = Arc::new(deepseekcode_core::permission::PermissionService::default());
    let err = permissions
        .ensure_allowed(
            SessionId::new(),
            "bash",
            &AgentKind::Plan,
            serde_json::json!({ "command": "echo hi" }),
        )
        .await
        .err();
    assert!(err.is_some());
}

#[tokio::test]
async fn plan_agent_blocks_write_outside_plan_dir() {
    let permissions = Arc::new(PolicyService::new(
        &DeepSeekCodeConfig::default(),
        "/tmp/project",
        None,
    ));
    let session_id = SessionId::new();
    let outcome = permissions
        .ensure_permitted(
            session_id,
            &AgentKind::Plan,
            "write",
            serde_json::json!({ "path": "src/main.rs", "content": "x" }),
            "/tmp/project",
        )
        .await
        .unwrap();
    assert!(matches!(outcome, PolicyOutcome::Blocked { .. }));
}

#[tokio::test]
async fn plan_agent_permits_write_in_plan_dir() {
    let config = DeepSeekCodeConfig {
        policy_mode: Some(PolicyMode::Auto),
        ..Default::default()
    };
    let permissions = Arc::new(PolicyService::new(&config, "/tmp/project", None));
    let session_id = SessionId::new();
    let outcome = permissions
        .ensure_permitted(
            session_id,
            &AgentKind::Plan,
            "write",
            serde_json::json!({
                "path": ".deepseekcode/plan/test-plan/PLAN.md",
                "content": "x"
            }),
            "/tmp/project",
        )
        .await
        .unwrap();
    assert!(!matches!(outcome, PolicyOutcome::Blocked { .. }));
}

#[tokio::test]
async fn plan_agent_permits_absolute_write_in_plan_dir() {
    let config = DeepSeekCodeConfig {
        policy_mode: Some(PolicyMode::Auto),
        ..Default::default()
    };
    let project = std::env::temp_dir().join("deepseekcode-plan-policy-test");
    let _ = std::fs::create_dir_all(&project);
    let project_dir = project.to_string_lossy().into_owned();
    let permissions = Arc::new(PolicyService::new(&config, project_dir.clone(), None));
    let session_id = SessionId::new();
    let abs = project
        .join(".deepseekcode")
        .join("plan")
        .join("test-plan")
        .join("PLAN.md");
    let outcome = permissions
        .ensure_permitted(
            session_id,
            &AgentKind::Plan,
            "write",
            serde_json::json!({
                "path": abs.to_string_lossy().to_string(),
                "content": "x"
            }),
            &project_dir,
        )
        .await
        .unwrap();
    assert!(!matches!(outcome, PolicyOutcome::Blocked { .. }));
}

#[tokio::test]
async fn plan_complete_discovers_existing_plan_dir() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(
        Database::open(&format!("sqlite://{}", db_path.display()))
            .await
            .unwrap(),
    );
    let events = Arc::new(EventStore::new(db.pool().clone()));
    let plan_dir = dir.path().join(".deepseekcode/plan/UI-redesign");
    std::fs::create_dir_all(&plan_dir).unwrap();
    let plan_body = "a".repeat(220);
    std::fs::write(plan_dir.join("PLAN.md"), &plan_body).unwrap();

    let session_id = SessionId::new();
    let services = ToolServices::default().with_events(events.clone());
    let ctx = ToolContext {
        session_id,
        agent: "plan".into(),
        assistant_message_id: MessageId::new(),
        tool_call_id: ToolCallId::new(),
        working_directory: dir.path().to_string_lossy().to_string(),
        extras: Default::default(),
    };
    let tool = PlanCompleteTool;
    let output = with_services_async(services, async {
        tool.execute(
            serde_json::json!({
                "title": "UI redesign plan",
            }),
            &ctx,
        )
        .await
        .expect("plan_complete should discover UI-redesign plan")
    })
    .await;
    assert_eq!(output["plan_id"], "UI-redesign");
    assert_eq!(output["status"], "completed");

    let tail = events
        .tail_filtered(0, 100, Some(session_id))
        .await
        .unwrap();
    assert!(tail.iter().any(|e| e.kind == EventKind::PlanCompleted));
}

#[tokio::test]
async fn plan_complete_emits_event_and_validates_file() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Arc::new(
        Database::open(&format!("sqlite://{}", db_path.display()))
            .await
            .unwrap(),
    );
    let events = Arc::new(EventStore::new(db.pool().clone()));
    let plan_dir = dir.path().join(".deepseekcode/plan/my-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    let plan_body = "a".repeat(220);
    std::fs::write(plan_dir.join("PLAN.md"), &plan_body).unwrap();

    let session_id = SessionId::new();
    let services = ToolServices::default().with_events(events.clone());
    let ctx = ToolContext {
        session_id,
        agent: "plan".into(),
        assistant_message_id: MessageId::new(),
        tool_call_id: ToolCallId::new(),
        working_directory: dir.path().to_string_lossy().to_string(),
        extras: Default::default(),
    };
    let tool = PlanCompleteTool;
    let output = with_services_async(services, async {
        tool.execute(
            serde_json::json!({
                "title": "My Plan",
                "plan_id": "my-plan",
                "summary": "summary"
            }),
            &ctx,
        )
        .await
        .expect("plan_complete should succeed")
    })
    .await;
    assert_eq!(output["status"], "completed");

    let tail = events
        .tail_filtered(0, 100, Some(session_id))
        .await
        .unwrap();
    assert!(tail.iter().any(|e| e.kind == EventKind::PlanCompleted));
}
