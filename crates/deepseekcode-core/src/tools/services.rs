//! Runtime tool services accessible during `Tool::execute` (set by ToolRegistry).

use async_trait::async_trait;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::tool::ToolFailure;
use deepseekcode_types::{Result, SessionId, ToolCallId};
use std::sync::Arc;

use deepseekcode_storage::EventStore;
use std::sync::atomic::{AtomicBool, Ordering};

tokio::task_local! {
    static TOOL_SERVICES: ToolServices;
    static TOOL_SESSION_ID: SessionId;
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

pub fn current_session_id() -> Option<SessionId> {
    TOOL_SESSION_ID.try_with(|id| *id).ok()
}

pub async fn with_session_id_async<F, R>(session_id: SessionId, f: F) -> R
where
    F: std::future::Future<Output = R>,
{
    TOOL_SESSION_ID.scope(session_id, f).await
}

pub fn ensure_not_cancelled() -> std::result::Result<(), ToolFailure> {
    let cancel = current_services().and_then(|s| s.cancel);
    if is_cancelled(cancel.as_ref()) {
        return Err(ToolFailure {
            message: "tool execution interrupted".into(),
            retryable: false,
        });
    }
    Ok(())
}

/// Cooperative cancellation primitive shared between the session runner and
/// tool execution. Unlike a bare `AtomicBool`, awaiting [`CancelToken::cancelled`]
/// parks the task on a `Notify` instead of busy-polling the flag.
#[derive(Default)]
pub struct CancelToken {
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Sets the flag and wakes every task awaiting [`CancelToken::cancelled`].
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Clears the flag so the token can be reused for a fresh run cycle.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }

    /// Resolves once `cancel()` has been called (returns immediately if it has).
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.notify.notified();
        tokio::pin!(notified);
        // Register as a waiter *before* re-checking the flag so a cancel()
        // racing between the check and the await cannot be missed.
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

pub async fn run_blocking_if_cancelled<F, T>(f: F) -> std::result::Result<T, ToolFailure>
where
    F: FnOnce() -> std::result::Result<T, ToolFailure> + Send + 'static,
    T: Send + 'static,
{
    ensure_not_cancelled()?;
    let cancel = current_services().and_then(|s| s.cancel);
    let handle = tokio::task::spawn_blocking(f);
    // The blocking closure cannot be aborted once started; on cancellation we
    // stop waiting for it and report the interruption.
    if let Some(token) = cancel {
        tokio::select! {
            result = handle => result.map_err(|e| ToolFailure {
                message: e.to_string(),
                retryable: true,
            })?,
            _ = token.cancelled() => {
                return Err(ToolFailure {
                    message: "tool execution interrupted".into(),
                    retryable: false,
                });
            }
        }
    } else {
        handle.await.map_err(|e| ToolFailure {
            message: e.to_string(),
            retryable: true,
        })?
    }
}

pub async fn race_cancel<F, T>(fut: F) -> std::result::Result<T, ToolFailure>
where
    F: std::future::Future<Output = std::result::Result<T, ToolFailure>>,
{
    let cancel = current_services().and_then(|s| s.cancel);
    if let Some(token) = cancel {
        tokio::select! {
            result = fut => result,
            _ = token.cancelled() => Err(ToolFailure {
                message: "tool execution interrupted".into(),
                retryable: false,
            }),
        }
    } else {
        fut.await
    }
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
    pub cancel: Option<Arc<CancelToken>>,
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

    pub fn with_cancel(mut self, cancel: Arc<CancelToken>) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

pub fn is_cancelled(cancel: Option<&Arc<CancelToken>>) -> bool {
    cancel.map(|token| token.is_cancelled()).unwrap_or(false)
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_resolves_immediately_when_already_cancelled() {
        let token = Arc::new(CancelToken::new());
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(100), token.cancelled())
            .await
            .expect("cancelled() must resolve at once");
    }

    #[tokio::test]
    async fn cancelled_wakes_on_cancel_without_busy_polling() {
        let token = Arc::new(CancelToken::new());
        let waiter = {
            let token = token.clone();
            tokio::spawn(async move {
                token.cancelled().await;
                true
            })
        };
        // Give the waiter a chance to register, then cancel it.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "waiter must still be parked");
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(500), waiter)
            .await
            .expect("waiter must wake")
            .expect("task join");
    }

    #[tokio::test]
    async fn cancel_racing_with_registration_is_never_missed() {
        for _ in 0..200 {
            let token = Arc::new(CancelToken::new());
            let waiter = {
                let token = token.clone();
                tokio::spawn(async move {
                    token.cancelled().await;
                })
            };
            token.cancel();
            tokio::time::timeout(std::time::Duration::from_millis(500), waiter)
                .await
                .expect("waiter must resolve even when cancel races registration")
                .expect("task join");
            token.reset();
        }
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
