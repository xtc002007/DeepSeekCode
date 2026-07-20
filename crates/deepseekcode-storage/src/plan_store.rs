use deepseekcode_types::{Result, SessionId};
use sqlx::SqlitePool;
use std::collections::HashSet;

pub struct PlanStore {
    pool: SqlitePool,
}

impl PlanStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_dismissed(
        &self,
        session_id: SessionId,
        plan_id: &str,
        dismissed: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO session_plan_ui (session_id, plan_id, implement_prompt_dismissed)
               VALUES (?, ?, ?)
               ON CONFLICT(session_id, plan_id) DO UPDATE SET
                 implement_prompt_dismissed = excluded.implement_prompt_dismissed"#,
        )
        .bind(session_id.to_string())
        .bind(plan_id)
        .bind(if dismissed { 1 } else { 0 })
        .execute(&self.pool)
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn list_dismissed(&self, session_id: SessionId) -> Result<HashSet<String>> {
        let rows = sqlx::query_as::<_, DismissRow>(
            r#"SELECT plan_id FROM session_plan_ui
               WHERE session_id = ? AND implement_prompt_dismissed = 1"#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| deepseekcode_types::DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.plan_id).collect())
    }
}

#[derive(sqlx::FromRow)]
struct DismissRow {
    plan_id: String,
}
