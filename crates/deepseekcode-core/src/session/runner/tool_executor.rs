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
        if self.interrupted.is_cancelled() {
            return Err(DeepSeekCodeError::SessionInterrupted);
        }

        let tool_call_id = provider_tool_call_id(&tool_call);
        let internal_tool_call_id =
            ToolCallId::from_str(&tool_call_id).unwrap_or_else(|_| ToolCallId::new());

        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
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
                // SessionInterrupted (e.g. interrupt during policy prompt)
                // must propagate, not masquerade as a policy denial (P0-4).
                if matches!(e, DeepSeekCodeError::SessionInterrupted) {
                    return Err(e);
                }
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

        // Read-lock only across execute (`execute(&self)` needs no &mut);
        // refresh paths take the write lock, so long tools no longer starve
        // definitions/refresh.
        let result_id = MessageId::new();
        let envelope: ToolResultEnvelope = match self
            .tools
            .read()
            .await
            .execute(
                &tool_call.name,
                normalized_input.clone(),
                self.session_id,
                assistant_id,
                internal_tool_call_id,
                &format!("{:?}", session.agent).to_lowercase(),
                Some(self.hooks.as_ref()),
                Some(self.interrupted.clone()),
            )
            .await
        {
            Ok(value) => value,
            Err(DeepSeekCodeError::ToolFailure(msg)) => {
                if self.interrupted.is_cancelled() {
                    let envelope = ToolResultEnvelope::new(
                        format!("{} interrupted", tool_call.name),
                        json!({
                            "error": msg,
                            "success": false,
                            "interrupted": true,
                        }),
                    )
                    .with_duration_ms(start.elapsed().as_millis() as u64);
                    return self
                        .complete_tool_interrupted(
                            &tool_call,
                            &tool_call_id,
                            assistant_id,
                            start,
                            envelope,
                        )
                        .await;
                }
                tracing::warn!(
                    target: deepseekcode_telemetry::TOOL,
                    run_id = %deepseekcode_telemetry::current_run_id(),
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

        if self.interrupted.is_cancelled() {
            return self
                .complete_tool_interrupted(&tool_call, &tool_call_id, assistant_id, start, envelope)
                .await;
        }

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

        // Success resets the loop-guard streak for this fingerprint so the
        // same tool reuse across drains does not accumulate (P0-4).
        {
            let normalized = deepseekcode_types::normalize_tool_arguments(&tool_call.arguments);
            let fp = tool_fingerprint(
                &tool_call.name,
                &serde_json::to_value(&normalized).unwrap_or(tool_call.arguments.clone()),
            );
            self.note_tool_success(&fp).await;
        }

        tracing::info!(
            target: deepseekcode_telemetry::TOOL,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %self.session_id,
            tool = %tool_call.name,
            tool_call_id = %tool_call_id,
            duration_ms = start.elapsed().as_millis(),
            "tool execution completed"
        );

        Ok(())
    }

    async fn complete_tool_interrupted(
        &self,
        tool_call: &LlmToolCall,
        tool_call_id: &str,
        _assistant_id: MessageId,
        start: std::time::Instant,
        envelope: ToolResultEnvelope,
    ) -> Result<()> {
        let interrupted = ToolResultEnvelope::new(
            envelope.title,
            json!({
                "error": "tool execution interrupted",
                "success": false,
                "interrupted": true,
                "partial_output": envelope.output,
            }),
        )
        .with_duration_ms(start.elapsed().as_millis() as u64);

        let result_id = MessageId::new();
        let stored = interrupted.to_stored();
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
                    "title": interrupted.title,
                    "metadata": interrupted.metadata,
                    "duration_ms": interrupted.duration_ms,
                    "output": output,
                }),
            )
            .await?;

        let _ = crate::activity::emit_activity_updated(&self.events, self.session_id, 0).await;
        Err(DeepSeekCodeError::SessionInterrupted)
    }

    async fn check_loop_guard(
        &self,
        session: &deepseekcode_schema::session::Session,
        tool_call: &LlmToolCall,
    ) -> Result<Option<String>> {
        use deepseekcode_schema::policy::PolicyDecision;

        // Normalize arguments (sorted keys, trimmed) so key-order/whitespace
        // variants cannot bypass the fingerprint. Count per-fingerprint in a
        // map so `A,A,B,A,A` still trips on the third consecutive A-run?
        // Spec: same tool+args repeated 3x consecutively. Non-consecutive
        // different tools reset only that fingerprint's streak, not all.
        let normalized = deepseekcode_types::normalize_tool_arguments(&tool_call.arguments);
        let fp = tool_fingerprint(
            &tool_call.name,
            &serde_json::to_value(&normalized).unwrap_or(tool_call.arguments.clone()),
        );
        // Turn-scoped counting: `loop_guard_turn` bumps on every user
        // promotion (see `promote_at_safe_boundary`). Same fp in the same
        // turn increments; a new turn or a different fp resets. Successful
        // tool completion resets via `note_tool_success` below.
        let turn = self
            .loop_guard_turn
            .load(std::sync::atomic::Ordering::SeqCst);
        let count = {
            let mut guard = self.loop_guard_counts.lock().await;
            let others: Vec<String> = guard
                .keys()
                .filter(|k| k.as_str() != fp.as_str())
                .cloned()
                .collect();
            for k in others {
                guard.remove(&k);
            }
            let entry = guard.entry(fp.clone()).or_insert((0u32, turn));
            if entry.1 != turn {
                *entry = (0, turn);
            }
            entry.0 = entry.0.saturating_add(1);
            entry.0
        };
        if count < 3 {
            return Ok(None);
        }

        // Evaluate with the concrete tool pattern so custom
        // `loop_guard:{tool:action}` rules hit (previously always `*`).
        let tool_pattern = format!("{}:*", tool_call.name);
        let patterns = vec![tool_pattern.clone(), "*".to_string()];
        match self
            .permissions
            .evaluate(self.session_id, &session.agent, "loop_guard", &patterns)
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
                        "pattern": tool_pattern,
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

    #[allow(clippy::too_many_arguments)]
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
