use super::SessionRunner;
use deepseekcode_schema::event::EventKind;
use deepseekcode_types::{DeepSeekCodeError, MessageId, Result};
use serde_json::json;

impl SessionRunner {
    pub(super) async fn reconcile_context(&self) -> Result<()> {
        let mut try_restore = true;
        loop {
            let epoch = self
                .context_epochs
                .get(self.session_id)
                .await?
                .ok_or_else(|| DeepSeekCodeError::ContextUnavailable("no epoch".into()))?;

            match self.context_registry.reconcile(&epoch.snapshot) {
                crate::system_context::ReconcileResult::Unchanged => return Ok(()),
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
                    return Ok(());
                }
                crate::system_context::ReconcileResult::ReplacementReady { generation } => {
                    tracing::debug!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %self.session_id,
                        baseline_len = generation.baseline.len(),
                        "context baseline replaced"
                    );
                    let baseline_seq = self.events.latest_seq(self.session_id).await?;
                    self.context_epochs
                        .replace_baseline(self.session_id, &generation, baseline_seq)
                        .await?;
                    return Ok(());
                }
                crate::system_context::ReconcileResult::ReplacementBlocked => {
                    tracing::warn!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %self.session_id,
                        "context replacement blocked"
                    );
                    if try_restore && crate::project_context::ensure_agents_md(&self.project_dir) {
                        tracing::info!(
                            target: deepseekcode_telemetry::SESSION,
                            session_id = %self.session_id,
                            "restored AGENTS.md after context reconciliation failure"
                        );
                        try_restore = false;
                        continue;
                    }
                    return Err(DeepSeekCodeError::ContextUnavailable("replacement blocked".into()));
                }
            }
        }
    }

    pub(super) async fn prepare_context_epoch(&self) -> Result<(String, i64)> {
        let epoch = self
            .context_epochs
            .get(self.session_id)
            .await?
            .ok_or_else(|| DeepSeekCodeError::ContextUnavailable("no epoch".into()))?;

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

        let mut try_restore = true;
        loop {
            let epoch = self
                .context_epochs
                .get(self.session_id)
                .await?
                .ok_or_else(|| DeepSeekCodeError::ContextUnavailable("no epoch".into()))?;

            match self.context_registry.reconcile(&epoch.snapshot) {
                crate::system_context::ReconcileResult::Unchanged => {
                    return Ok((epoch.baseline, epoch.baseline_seq));
                }
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
                        .ok_or_else(|| DeepSeekCodeError::ContextUnavailable("no epoch".into()))?;
                    let baseline = self
                        .context_registry
                        .render_baseline()
                        .map(|g| g.baseline)
                        .unwrap_or(epoch.baseline);
                    return Ok((baseline, epoch.baseline_seq));
                }
                crate::system_context::ReconcileResult::ReplacementReady { generation } => {
                    let baseline = generation.baseline.clone();
                    let baseline_seq = self.events.latest_seq(self.session_id).await?;
                    self.context_epochs
                        .replace_baseline(self.session_id, &generation, baseline_seq)
                        .await?;
                    return Ok((baseline, baseline_seq));
                }
                crate::system_context::ReconcileResult::ReplacementBlocked => {
                    tracing::warn!(
                        target: deepseekcode_telemetry::SESSION,
                        session_id = %self.session_id,
                        "context replacement blocked"
                    );
                    if try_restore && crate::project_context::ensure_agents_md(&self.project_dir) {
                        tracing::info!(
                            target: deepseekcode_telemetry::SESSION,
                            session_id = %self.session_id,
                            "restored AGENTS.md after context reconciliation failure"
                        );
                        try_restore = false;
                        continue;
                    }
                    return Err(DeepSeekCodeError::ContextUnavailable("replacement blocked".into()));
                }
            }
        }
    }
}
