use async_trait::async_trait;
use deepseekcode_schema::todo::TodoItem;
use deepseekcode_schema::tool::TodoStoreOps;
use deepseekcode_types::{Result, SessionId};
use std::sync::Arc;

#[async_trait]
pub trait TodoStore: Send + Sync {
    async fn update(&self, session_id: SessionId, todos: &[TodoItem]) -> Result<Vec<TodoItem>>;
    async fn get(&self, session_id: SessionId) -> Result<Vec<TodoItem>>;
}

#[async_trait]
impl TodoStore for deepseekcode_storage::TodoStore {
    async fn update(&self, session_id: SessionId, todos: &[TodoItem]) -> Result<Vec<TodoItem>> {
        deepseekcode_storage::TodoStore::update(self, session_id, todos).await
    }

    async fn get(&self, session_id: SessionId) -> Result<Vec<TodoItem>> {
        deepseekcode_storage::TodoStore::get(self, session_id).await
    }
}

/// Bridges storage `TodoStore` into schema `TodoStoreOps` for `ToolContext`.
pub struct StorageTodoStoreOps(pub Arc<deepseekcode_storage::TodoStore>);

#[async_trait]
impl TodoStoreOps for StorageTodoStoreOps {
    async fn update_todos(
        &self,
        session_id: SessionId,
        todos: &[TodoItem],
    ) -> std::result::Result<Vec<TodoItem>, String> {
        self.0
            .update(session_id, todos)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_todos(&self, session_id: SessionId) -> std::result::Result<Vec<TodoItem>, String> {
        self.0.get(session_id).await.map_err(|e| e.to_string())
    }
}
