//! Prompt admission at safe provider-turn boundaries.

use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::session::PromptDelivery;
use deepseekcode_storage::SessionStore;
use deepseekcode_types::{MessageId, Result, SessionId};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

use deepseekcode_storage::EventStore;
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
        if self.in_provider_turn.load(Ordering::SeqCst) {
            return self.promote_steer_if_any().await;
        }

        if !baseline_ready {
            return Ok(false);
        }

        let pending = self.sessions.pending_inputs(self.session_id).await?;
        let next = pending
            .iter()
            .find(|p| !p.promoted && matches!(p.delivery, PromptDelivery::Queue))
            .or_else(|| {
                pending
                    .iter()
                    .find(|p| !p.promoted && matches!(p.delivery, PromptDelivery::Steer))
            });
        let Some(input) = next else {
            return Ok(false);
        };
        self.promote_input(input.id, &input.prompt).await
    }

    pub async fn promote_steer_if_any(&self) -> Result<bool> {
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
        self.sessions
            .insert_message(message_id, self.session_id, "user", prompt)
            .await?;
        self.sessions.promote_input(message_id).await?;
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
