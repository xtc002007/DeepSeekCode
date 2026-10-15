//! Prompt admission at safe provider-turn boundaries.

use code_plus_plus_schema::event::EventKind;
use code_plus_plus_schema::session::PromptDelivery;
use code_plus_plus_storage::SessionStore;
use code_plus_plus_types::{MessageId, Result, SessionId};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

use code_plus_plus_storage::EventStore;
use std::sync::Arc;

pub struct PromptPromoter<'a> {
    session_id: SessionId,
    sessions: &'a SessionStore,
    events: Arc<EventStore>,
    in_provider_turn: &'a AtomicBool,
}

impl<'a> PromptPromoter<'a> {
    pub fn new(
        session_id: SessionId,
        sessions: &'a SessionStore,
        events: Arc<EventStore>,
        in_provider_turn: &'a AtomicBool,
    ) -> Self {
        Self {
            session_id,
            sessions,
            events,
            in_provider_turn,
        }
    }

    pub async fn promote_at_safe_boundary(&self, baseline_ready: bool) -> Result<bool> {
        tracing::debug!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %self.session_id,
            baseline_ready = baseline_ready,
            "prompt promote_at_safe_boundary started"
        );
        if self.in_provider_turn.load(Ordering::SeqCst) {
            return self.promote_steer_if_any().await;
        }

        if !baseline_ready {
            return Ok(false);
        }

        // Idle safe boundary: promote ALL pending inputs in admission order
        // (queue and steer both visible). Previously only one row per drain
        // was promoted and queue starved steer, requiring multiple drains.
        let pending = self.sessions.pending_inputs(self.session_id).await?;
        let mut any = false;
        for input in pending.iter().filter(|p| !p.promoted) {
            self.promote_input(input.id, &input.prompt).await?;
            any = true;
        }
        Ok(any)
    }

    pub async fn promote_steer_if_any(&self) -> Result<bool> {
        tracing::debug!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %self.session_id,
            "prompt promote_steer_if_any started"
        );
        let pending = self.sessions.pending_inputs(self.session_id).await?;
        let next = pending
            .into_iter()
            .find(|p| !p.promoted && matches!(p.delivery, PromptDelivery::Steer));
        let Some(input) = next else {
            return Ok(false);
        };
        self.promote_input(input.id, &input.prompt).await
    }

    async fn promote_input(&self, message_id: MessageId, prompt: &str) -> Result<bool> {
        tracing::info!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %self.session_id,
            message_id = %message_id,
            prompt = %code_plus_plus_telemetry::summarize_prompt(prompt, false),
            "prompt promoted"
        );
        self.sessions
            .promote_input_atomic(message_id, self.session_id, prompt)
            .await?;
        self.events
            .append(
                EventKind::Prompted,
                Some(self.session_id),
                json!({ "message_id": message_id.to_string() }),
            )
            .await?;
        Ok(true)
    }
}
