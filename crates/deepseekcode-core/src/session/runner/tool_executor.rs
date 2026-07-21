use super::repair::provider_tool_call_id;
use super::SessionRunner;
use deepseekcode_llm::LlmToolCall;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::tool::ToolResultEnvelope;
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result, ToolCallId};
use serde_json::json;
use std::str::FromStr;

use crate::permission::PolicyOutcome;
use crate::policy::patterns::tool_fingerprint;
use crate::tools::registry::preview_tool_start;

impl SessionRunner {
    pub(super) async fn execute_tool(
        &self,
        session: &deepseekcode_schema::session::Session,
        tool_call: LlmToolCall,
        assistant_id: MessageId,
    ) -> Result<()> {
        let tool_call_id = provider_tool_call_id(&tool_call);
        let internal_tool_call_id =
            ToolCallId::from_str(&tool_call_id).unwrap_or_else(|_| ToolCallId::new());

        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            session_id = %self.session_id,
            tool = %tool_call.name,
            tool_call_id = %tool_call_id,
            "tool execution started"
        );
        let start = std::time::Instant::now();

        if let Some(msg) = self.check_loop_guard(session, &tool_call).await? {
            return self
                .complete_tool_policy_failure(
                    session,
                    &tool_call,
                    &tool_call_id,
                    internal_tool_call_id,
                    assistant_id,
                    start,
                    msg,
                )
                .await;
        }

        match self
            .permissions
            .ensure_permitted(
                self.session_id,
                &session.agent,
                &tool_call.name,
                tool_call.arguments.clone(),
                &self.project_dir,
            )
            .await
        {
            Ok(PolicyOutcome::Permitted) => {}
            Ok(PolicyOutcome::Blocked { reason })
            | Ok(PolicyOutcome::Rejected {
                message: Some(reason),
            }) => {
                return self
                    .complete_tool_policy_failure(
                        session,
                        &tool_call,
                        &tool_call_id,
                        internal_tool_call_id,
                        assistant_id,
                        start,
                        reason,
                    )
                    .await;
            }
            Ok(PolicyOutcome::Rejected { message: None }) => {
                return self
                    .complete_tool_policy_failure(
                        session,
                        &tool_call,
                        &tool_call_id,
                        internal_tool_call_id,
                        assistant_id,
                        start,
                        format!("{} denied by user", tool_call.name),
                    )
                    .await;
            }
            Err(e) => {
                return self
                    .complete_tool_policy_failure(
                        session,
                        &tool_call,
                        &tool_call_id,
                        internal_tool_call_id,
                        assistant_id,
                        start,
                        e.to_string(),
                    )
                    .await;
            }
        }

        let normalized_input = deepseekcode_types::normalize_tool_arguments(&tool_call.arguments);
        let (preview_title, preview_metadata) =
            preview_tool_start(&tool_call.name, &normalized_input);

        self.events
            .append(
                EventKind::ToolStarted,
                Some(self.session_id),
                json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_call.name,
                    "input": normalized_input,
                    "title": preview_title,
                    "metadata": preview_metadata,
                }),
            )
            .await?;

        let result_id = MessageId::new();
        let envelope: ToolResultEnvelope = match self
            .tools
            .lock()
            .await
            .execute(
                &tool_call.name,
                normalized_input.clone(),
                self.session_id,
                assistant_id,
                internal_tool_call_id,
                &format!("{:?}", session.agent).to_lowercase(),
                Some(self.hooks.as_ref()),
            )
            .await
        {
            Ok(value) => value,
            Err(DeepSeekCodeError::ToolFailure(msg)) => {
                tracing::warn!(
                    target: deepseekcode_telemetry::TOOL,
                    session_id = %self.session_id,
                    tool = %tool_call.name,
                    tool_call_id = %tool_call_id,
                    error = %msg,
                    "tool returned failure to model"
                );
                ToolResultEnvelope::new(
                    format!("{} failed", tool_call.name),
                    json!({ "error": msg, "success": false }),
                )
                .with_duration_ms(start.elapsed().as_millis() as u64)
            }
            Err(e) => return Err(e),
        };

        let stored = envelope.to_stored();
        let output = self.tool_output.store(
            self.session_id,
            &tool_call_id,
            &tool_call.name,
            &stored,
            &self.tool_output_limits(),
        )?;

        self.sessions
            .insert_tool_result(
                result_id,
                self.session_id,
                0,
                &tool_call_id,
                &tool_call.name,
                &output,
            )
            .await?;

        self.events
            .append(
                EventKind::ToolCompleted,
                Some(self.session_id),
                json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_call.name,
                    "title": envelope.title,
                    "metadata": envelope.metadata,
                    "duration_ms": envelope.duration_ms,
                    "output": output,
                }),
            )
            .await?;

        let _ = crate::activity::emit_activity_updated(&self.events, self.session_id, 0).await;

        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            session_id = %self.session_id,
            tool = %tool_call.name,
            tool_call_id = %tool_call_id,
            duration_ms = start.elapsed().as_millis(),
            "tool execution completed"
        );

        Ok(())
    }

    async fn check_loop_guard(
        &self,
        session: &deepseekcode_schema::session::Session,
        tool_call: &LlmToolCall,
    ) -> Result<Option<String>> {
        use deepseekcode_schema::policy::PolicyDecision;

        let fp = tool_fingerprint(&tool_call.name, &tool_call.arguments);
        let count = {
            let mut guard = self.loop_guard.lock().await;
            let count = match guard.as_ref() {
                Some((prev, c)) if prev == &fp => c.saturating_add(1),
                _ => 1,
            };
            *guard = Some((fp, count));
            count
        };
        if count < 3 {
            return Ok(None);
        }

        match self
            .permissions
            .evaluate(
                self.session_id,
                &session.agent,
                "loop_guard",
                &["*".to_string()],
            )
            .await
        {
            PolicyDecision::Permit => Ok(None),
            PolicyDecision::Block => Ok(Some(
                "loop guard blocked repeated identical tool invocation".into(),
            )),
            PolicyDecision::Prompt => match self
                .permissions
                .ensure_permitted(
                    self.session_id,
                    &session.agent,
                    "loop_guard",
                    json!({
                        "tool": tool_call.name,
                        "count": count,
                    }),
                    &self.project_dir,
                )
                .await
            {
                Ok(PolicyOutcome::Permitted) => Ok(None),
                Ok(PolicyOutcome::Blocked { reason }) => Ok(Some(reason)),
                Ok(PolicyOutcome::Rejected { message }) => Ok(Some(
                    message.unwrap_or_else(|| "loop guard denied by user".into()),
                )),
                Err(e) => Ok(Some(e.to_string())),
            },
        }
    }

    async fn complete_tool_policy_failure(
        &self,
        _session: &deepseekcode_schema::session::Session,
        tool_call: &LlmToolCall,
        tool_call_id: &str,
        _internal_tool_call_id: ToolCallId,
        _assistant_id: MessageId,
        start: std::time::Instant,
        message: String,
    ) -> Result<()> {
        let normalized_input = deepseekcode_types::normalize_tool_arguments(&tool_call.arguments);
        let (preview_title, preview_metadata) =
            preview_tool_start(&tool_call.name, &normalized_input);

        self.events
            .append(
                EventKind::ToolStarted,
                Some(self.session_id),
                json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_call.name,
                    "input": normalized_input,
                    "title": preview_title,
                    "metadata": preview_metadata,
                }),
            )
            .await?;

        let envelope = ToolResultEnvelope::new(
            format!("{} blocked", tool_call.name),
            json!({ "error": message, "success": false, "policy": true }),
        )
        .with_duration_ms(start.elapsed().as_millis() as u64);

        let result_id = MessageId::new();
        let stored = envelope.to_stored();
        let output = self.tool_output.store(
            self.session_id,
            tool_call_id,
            &tool_call.name,
            &stored,
            &self.tool_output_limits(),
        )?;

        self.sessions
            .insert_tool_result(
                result_id,
                self.session_id,
                0,
                tool_call_id,
                &tool_call.name,
                &output,
            )
            .await?;

        self.events
            .append(
                EventKind::ToolCompleted,
                Some(self.session_id),
                json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_call.name,
                    "title": envelope.title,
                    "metadata": envelope.metadata,
                    "duration_ms": envelope.duration_ms,
                    "output": output,
                }),
            )
            .await?;

        let _ = crate::activity::emit_activity_updated(&self.events, self.session_id, 0).await;
        Ok(())
    }
}
