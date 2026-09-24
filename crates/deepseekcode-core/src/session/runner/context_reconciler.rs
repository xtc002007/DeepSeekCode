use super::SessionRunner;
use deepseekcode_schema::event::EventKind;
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result};
use serde_json::json;

impl SessionRunner {
    /// Shared reconcile core for `reconcile_context` (fire-and-apply) and
    /// `prepare_context_epoch` (apply-and-return). Previously ~80 lines were
    /// duplicated and drifted (P0-7); all branches now go through
    /// `apply_reconcile_once`.
    async fn apply_reconcile_once(
        &self,
        epoch_snapshot: &crate::system_context::Snapshot,
    ) -> Result<Option<(String, i64)>> {
        match self.context_registry.reconcile(epoch_snapshot) {
            crate::system_context::ReconcileResult::Unchanged => Ok(None),
            crate::system_context::ReconcileResult::Updated { text, snapshot } => {
                tracing::debug!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    "context updated"
                );
                let system_id = MessageId::new();
                self.sessions
                    .insert_message(system_id, self.session_id, "system", &text)
                    .await?;
                self.context_epochs
                    .update_snapshot(self.session_id, &snapshot)
                    .await?;
                self.events
                    .append(
                        EventKind::ContextUpdated,
                        Some(self.session_id),
                        json!({ "text": text }),
                    )
                    .await?;
                let epoch = self
                    .context_epochs
                    .get(self.session_id)
                    .await?
                    .ok_or_else(|| {
                        DeepSeekCodeError::ContextUnavailable(format!(
                            "context epoch missing for session {} (Rule 2: mandatory context unavailable; prompt stays pending)",
                            self.session_id
                        ))
                    })?;
                Ok(Some((epoch.baseline, epoch.baseline_seq)))
            }
            crate::system_context::ReconcileResult::ReplacementReady { generation } => {
                tracing::debug!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    baseline_len = generation.baseline.len(),
                    "context baseline replaced"
                );
                let baseline_seq = self.sessions.latest_message_seq(self.session_id).await?;
                let baseline = generation.baseline.clone();
                self.context_epochs
                    .replace_baseline(self.session_id, &generation, baseline_seq)
                    .await?;
                Ok(Some((baseline, baseline_seq)))
            }
            crate::system_context::ReconcileResult::ReplacementBlocked => {
                tracing::warn!(
                    target: deepseekcode_telemetry::SESSION,
                    session_id = %self.session_id,
                    "context replacement blocked"
                );
                // P0-7: never auto-create AGENTS.md here. Fabricating
                // mandatory context violates Spec Rule 2 ("Unavailable blocks
                // turn; prompt stays pending and retryable"). Surface a
                // distinct error so the drain keeps the prompt pending.
                Err(DeepSeekCodeError::ContextUnavailable(format!(
                    "mandatory context unavailable for session {} (Rule 2: add AGENTS.md or configure instructions, then retry)",
                    self.session_id
                )))
            }
        }
    }

    pub(super) async fn reconcile_context(&self) -> Result<()> {
        let epoch = self
            .context_epochs
            .get(self.session_id)
            .await?
            .ok_or_else(|| {
                DeepSeekCodeError::ContextUnavailable(format!(
                    "context epoch missing for session {} (Rule 1: baseline must initialize before any prompt is model-visible)",
                    self.session_id
                ))
            })?;
        let _ = self.apply_reconcile_once(&epoch.snapshot).await?;
        Ok(())
    }

    pub(super) async fn prepare_context_epoch(&self) -> Result<(String, i64)> {
        let epoch = self
            .context_epochs
            .get(self.session_id)
            .await?
            .ok_or_else(|| {
                DeepSeekCodeError::ContextUnavailable(format!(
                    "context epoch missing for session {} (Rule 1)",
                    self.session_id
                ))
            })?;

        let compaction_seq = self.sessions.latest_compaction_seq(self.session_id).await?;
        if let Some(baseline_seq) = compaction_seq.filter(|seq| *seq > epoch.baseline_seq) {
            if let Some(generation) = self.context_registry.render_baseline() {
                let baseline = generation.baseline.clone();
                self.context_epochs
                    .replace_baseline(self.session_id, &generation, baseline_seq)
                    .await?;
                return Ok((baseline, baseline_seq));
            }
        }

        let epoch = self
            .context_epochs
            .get(self.session_id)
            .await?
            .ok_or_else(|| {
                DeepSeekCodeError::ContextUnavailable(format!(
                    "context epoch missing for session {} (Rule 1)",
                    self.session_id
                ))
            })?;
        if let Some(updated) = self.apply_reconcile_once(&epoch.snapshot).await? {
            return Ok(updated);
        }
        Ok((epoch.baseline, epoch.baseline_seq))
    }
}
