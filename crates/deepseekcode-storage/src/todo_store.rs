use deepseekcode_schema::todo::TodoItem;
use deepseekcode_types::{DeepSeekCodeError, Result, SessionId};
use sqlx::SqlitePool;

pub struct TodoStore {
    pool: SqlitePool,
}

impl TodoStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn update(&self, session_id: SessionId, todos: &[TodoItem]) -> Result<Vec<TodoItem>> {
        let sid = session_id.to_string();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        sqlx::query("DELETE FROM session_todos WHERE session_id = ?")
            .bind(&sid)
            .execute(&mut *tx)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        for (position, todo) in todos.iter().enumerate() {
            sqlx::query(
                "INSERT INTO session_todos (session_id, position, content, status, priority) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&sid)
            .bind(position as i64)
            .bind(&todo.content)
            .bind(&todo.status)
            .bind(&todo.priority)
            .execute(&mut *tx)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        }
        tx.commit()
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        self.get(session_id).await
    }

    pub async fn get(&self, session_id: SessionId) -> Result<Vec<TodoItem>> {
        let rows = sqlx::query_as::<_, TodoRow>(
            "SELECT content, status, priority FROM session_todos WHERE session_id = ? ORDER BY position ASC",
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.into_item()).collect())
    }
}

#[derive(sqlx::FromRow)]
struct TodoRow {
    content: String,
    status: String,
    priority: String,
}

impl TodoRow {
    fn into_item(self) -> TodoItem {
        TodoItem {
            content: self.content,
            status: self.status,
            priority: self.priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[tokio::test]
    async fn todo_roundtrip() {
        let db = Database::open(":memory:").await.unwrap();
        let store = TodoStore::new(db.pool().clone());
        let sid = SessionId::new();
        let todos = vec![
            TodoItem {
                content: "task one".into(),
                status: "pending".into(),
                priority: "high".into(),
            },
            TodoItem {
                content: "task two".into(),
                status: "in_progress".into(),
                priority: "medium".into(),
            },
        ];
        store.update(sid, &todos).await.unwrap();
        let loaded = store.get(sid).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "task one");
    }
}
