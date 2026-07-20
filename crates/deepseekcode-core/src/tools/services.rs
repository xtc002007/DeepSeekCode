//! Runtime tool services accessible during `Tool::execute` (set by ToolRegistry).

use async_trait::async_trait;
use deepseekcode_schema::event::EventKind;
use deepseekcode_types::{Result, SessionId, ToolCallId};
use std::sync::Arc;

use deepseekcode_storage::EventStore;

tokio::task_local! {
    static TOOL_SERVICES: ToolServices;
}

pub fn with_services<R>(services: ToolServices, f: impl FnOnce() -> R) -> R {
    futures::executor::block_on(with_services_async(services, async { f() }))
}

pub async fn with_services_async<F, R>(services: ToolServices, f: F) -> R
where
    F: std::future::Future<Output = R>,
{
    TOOL_SERVICES.scope(services, f).await
}

pub fn current_services() -> Option<ToolServices> {
    TOOL_SERVICES.try_with(|services| services.clone()).ok()
}

/// Sink for tool progress events (bash streaming, etc.).
#[async_trait]
pub trait ToolProgressSink: Send + Sync {
    async fn progress(
        &self,
        session_id: SessionId,
        tool_call_id: &ToolCallId,
        tool_name: &str,
        chunk: &str,
        stream: &str,
    ) -> Result<()>;
}

pub struct EventToolProgressSink {
    events: Arc<EventStore>,
}

impl EventToolProgressSink {
    pub fn new(events: Arc<EventStore>) -> Self {
        Self { events }
    }
}

#[async_trait]
impl ToolProgressSink for EventToolProgressSink {
    async fn progress(
        &self,
        session_id: SessionId,
        tool_call_id: &ToolCallId,
        tool_name: &str,
        chunk: &str,
        stream: &str,
    ) -> Result<()> {
        self.events
            .append(
                EventKind::ToolProgress,
                Some(session_id),
                serde_json::json!({
                    "tool_call_id": tool_call_id.to_string(),
                    "tool_name": tool_name,
                    "chunk": chunk,
                    "stream": stream,
                }),
            )
            .await?;
        Ok(())
    }
}

/// Spawn a child session task (used by the `task` tool).
#[async_trait]
pub trait TaskSpawner: Send + Sync {
    async fn spawn_task(
        &self,
        parent_session_id: SessionId,
        agent: &str,
        prompt: &str,
    ) -> Result<SessionId>;
}

/// Runtime services passed to tool execution.
#[derive(Clone, Default)]
pub struct ToolServices {
    pub progress: Option<Arc<dyn ToolProgressSink>>,
    pub todo_store: Option<Arc<deepseekcode_storage::TodoStore>>,
    pub events: Option<Arc<EventStore>>,
    pub question_tx: Option<Arc<tokio::sync::Mutex<Option<QuestionResponder>>>>,
    pub task_spawner: Option<Arc<dyn TaskSpawner>>,
}

pub type QuestionResponder = tokio::sync::oneshot::Sender<String>;

impl ToolServices {
    pub fn with_progress(mut self, sink: Arc<dyn ToolProgressSink>) -> Self {
        self.progress = Some(sink);
        self
    }

    pub fn with_todo_store(mut self, store: Arc<deepseekcode_storage::TodoStore>) -> Self {
        self.todo_store = Some(store);
        self
    }

    pub fn with_events(mut self, events: Arc<EventStore>) -> Self {
        self.events = Some(events);
        self
    }

    pub fn with_task_spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self {
        self.task_spawner = Some(spawner);
        self
    }
}

pub async fn emit_todo_updated(
    services: &ToolServices,
    session_id: SessionId,
    todos: &[deepseekcode_schema::todo::TodoItem],
) -> Result<()> {
    if let Some(events) = &services.events {
        events
            .append(
                EventKind::TodoUpdated,
                Some(session_id),
                serde_json::json!({ "todos": todos }),
            )
            .await?;
    }
    Ok(())
}

pub async fn wait_for_question_answer(
    services: &ToolServices,
    timeout: std::time::Duration,
) -> Result<String> {
    let Some(tx_mutex) = services.question_tx.as_ref() else {
        return Err(deepseekcode_types::DeepSeekCodeError::ToolFailure(
            "question channel not configured".into(),
        ));
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    *tx_mutex.lock().await = Some(tx);
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(_)) => Err(deepseekcode_types::DeepSeekCodeError::ToolFailure(
            "question cancelled".into(),
        )),
        Err(_) => Err(deepseekcode_types::DeepSeekCodeError::ToolFailure(
            "question timed out".into(),
        )),
    }
}

pub fn submit_question_answer(services: &ToolServices, answer: String) -> Result<()> {
    let Some(tx_mutex) = services.question_tx.as_ref() else {
        return Err(deepseekcode_types::DeepSeekCodeError::NotFound(
            "no pending question".into(),
        ));
    };
    submit_question_answer_channel(tx_mutex, answer)
}

pub fn submit_question_answer_channel(
    tx_mutex: &Arc<tokio::sync::Mutex<Option<QuestionResponder>>>,
    answer: String,
) -> Result<()> {
    if let Ok(mut guard) = tx_mutex.try_lock() {
        if let Some(tx) = guard.take() {
            let _ = tx.send(answer);
            return Ok(());
        }
    }
    Err(deepseekcode_types::DeepSeekCodeError::NotFound(
        "no pending question".into(),
    ))
}

struct NoopTaskSpawner;

#[async_trait]
impl TaskSpawner for NoopTaskSpawner {
    async fn spawn_task(
        &self,
        _parent_session_id: SessionId,
        _agent: &str,
        _prompt: &str,
    ) -> Result<SessionId> {
        Err(deepseekcode_types::DeepSeekCodeError::Other(
            "task spawner not available".into(),
        ))
    }
}

pub fn noop_task_spawner() -> Arc<dyn TaskSpawner> {
    Arc::new(NoopTaskSpawner)
}
