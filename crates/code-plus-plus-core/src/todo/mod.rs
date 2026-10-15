use async_trait::async_trait;
use code_plus_plus_schema::todo::TodoItem;
use code_plus_plus_schema::tool::TodoStoreOps;
use code_plus_plus_types::{Result, SessionId};
use std::sync::Arc;

#[async_trait]
pub trait TodoStore: Send + Sync {
    async fn update(&self, session_id: SessionId, todos: &[TodoItem]) -> Result<Vec<TodoItem>>;
    async fn get(&self, session_id: SessionId) -> Result<Vec<TodoItem>>;
}

#[async_trait]
impl TodoStore for code_plus_plus_storage::TodoStore {
    async fn update(&self, session_id: SessionId, todos: &[TodoItem]) -> Result<Vec<TodoItem>> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            todos = todos.len(),
            "todo update started"
        );
        let r = code_plus_plus_storage::TodoStore::update(self, session_id, todos).await;
        match &r {
            Ok(saved) => tracing::info!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %session_id,
                todos = saved.len(),
                "todo update completed"
            ),
            Err(e) => tracing::error!(
                target: code_plus_plus_telemetry::TOOL,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                session_id = %session_id,
                error = %e,
                "todo update failed"
            ),
        }
        r
    }

    async fn get(&self, session_id: SessionId) -> Result<Vec<TodoItem>> {
        tracing::debug!(
            target: code_plus_plus_telemetry::TOOL,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            "todo get started"
        );
        code_plus_plus_storage::TodoStore::get(self, session_id).await
    }
}

/// Bridges storage `TodoStore` into schema `TodoStoreOps` for `ToolContext`.
pub struct StorageTodoStoreOps(pub Arc<code_plus_plus_storage::TodoStore>);

impl TodoStoreOps for StorageTodoStoreOps {
    fn update_todos<'a>(
        &'a self,
        session_id: SessionId,
        todos: &'a [TodoItem],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<Vec<TodoItem>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.0
                .update(session_id, todos)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn get_todos(
        &self,
        session_id: SessionId,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<Vec<TodoItem>, String>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move { self.0.get(session_id).await.map_err(|e| e.to_string()) })
    }
}
