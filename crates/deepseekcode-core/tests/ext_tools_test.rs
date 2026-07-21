use async_trait::async_trait;
use deepseekcode_core::tools::question::QuestionTool;
use deepseekcode_core::tools::registry::Tool;
use deepseekcode_core::tools::services::{submit_question_answer, ToolProgressSink, ToolServices};
use deepseekcode_core::tools::task::TaskTool;
use deepseekcode_types::{Result, SessionId, ToolCallId};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct MockTaskSpawner {
    child: SessionId,
}

#[async_trait]
impl deepseekcode_core::tools::services::TaskSpawner for MockTaskSpawner {
    async fn spawn_task(
        &self,
        _parent_session_id: SessionId,
        _agent: &str,
        _prompt: &str,
    ) -> Result<SessionId> {
        Ok(self.child)
    }
}

struct RecordingProgress {
    messages: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ToolProgressSink for RecordingProgress {
    async fn progress(
        &self,
        _session_id: SessionId,
        _tool_call_id: &ToolCallId,
        _tool_name: &str,
        chunk: &str,
        _stream: &str,
    ) -> Result<()> {
        self.messages.lock().unwrap().push(chunk.to_string());
        Ok(())
    }
}

#[tokio::test]
async fn task_tool_spawns_child_and_emits_progress() {
    let child = SessionId::new();
    let progress_msgs = Arc::new(Mutex::new(Vec::new()));
    let services = ToolServices::default()
        .with_task_spawner(Arc::new(MockTaskSpawner { child }))
        .with_progress(Arc::new(RecordingProgress {
            messages: progress_msgs.clone(),
        }));
    let ctx = deepseekcode_schema::tool::ToolContext {
        session_id: SessionId::new(),
        tool_call_id: ToolCallId::new(),
        agent: "build".into(),
        assistant_message_id: deepseekcode_types::MessageId::new(),
        working_directory: ".".into(),
        extras: Default::default(),
    };
    let out = deepseekcode_core::tools::services::with_services_async(services, async {
        TaskTool
            .execute(
                json!({ "agent": "build", "prompt": "do work", "title": "Work" }),
                &ctx,
            )
            .await
            .expect("task execute")
    })
    .await;
    assert_eq!(out["session_id"], child.to_string());
    assert_eq!(out["status"], "spawned");
    let msgs = progress_msgs.lock().unwrap();
    assert!(msgs.iter().any(|m| m.contains("spawning")));
    assert!(msgs.iter().any(|m| m.contains("running")));
}

#[tokio::test]
async fn question_tool_waits_for_submitted_answer() {
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
                .execute(
                    json!({ "question": "Pick one", "options": ["a", "b"] }),
                    &ctx,
                )
                .await
                .expect("question execute")
        })
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    submit_question_answer(
        &ToolServices {
            question_tx: Some(question_tx),
            ..Default::default()
        },
        "option-a".into(),
    )
    .expect("submit answer");
    let out = answer_task.await.expect("join");
    assert_eq!(out["answer"], "option-a");
}

#[tokio::test]
async fn bash_tool_streams_stdout_via_progress_sink() {
    use deepseekcode_core::tools::builtin::BashTool;
    use deepseekcode_core::tools::registry::Tool;
    let dir = tempfile::tempdir().expect("tempdir");
    let progress_msgs = Arc::new(Mutex::new(Vec::new()));
    let services = ToolServices::default().with_progress(Arc::new(RecordingProgress {
        messages: progress_msgs.clone(),
    }));
    let ctx = deepseekcode_schema::tool::ToolContext {
        session_id: SessionId::new(),
        tool_call_id: ToolCallId::new(),
        agent: "build".into(),
        assistant_message_id: deepseekcode_types::MessageId::new(),
        working_directory: dir.path().to_string_lossy().to_string(),
        extras: Default::default(),
    };
    let command = if cfg!(windows) {
        "cmd /c echo streamed-line"
    } else {
        "echo streamed-line"
    };
    deepseekcode_core::tools::services::with_services_async(services, async {
        BashTool::new(None, std::collections::HashMap::new())
            .execute(serde_json::json!({ "command": command }), &ctx)
            .await
            .expect("bash execute");
    })
    .await;
    let msgs = progress_msgs.lock().unwrap();
    assert!(
        msgs.iter().any(|m| m.contains("streamed-line")),
        "expected progress chunks, got {msgs:?}"
    );
}
