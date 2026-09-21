use super::epoch::{reconcile_snapshot, reconcile_snapshot_removed, ContextEpochStore};
use super::source::{
    agents_md_source, environment_source, instructions_source, skill_guidance_source,
    workspace_source, ContextSource, Generation, LoadResult, ReconcileResult, Snapshot,
};
use crate::skills::SkillCatalog;
use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_types::{Result, SessionId};

pub struct SystemContextRegistry {
    project_dir: String,
    sources: Vec<Box<dyn ErasedSource>>,
}

impl SystemContextRegistry {
    pub fn new(project_dir: impl Into<String>) -> Self {
        Self::with_config(project_dir, &DeepSeekCodeConfig::default())
    }

    pub fn with_config(project_dir: impl Into<String>, config: &DeepSeekCodeConfig) -> Self {
        let project_dir = project_dir.into();
        let skills = SkillCatalog::discover(
            &config.skills,
            &project_dir,
            config.skills_index_url.as_deref(),
        );
        let guidance = SkillCatalog::guidance_text(&skills);
        let sources: Vec<Box<dyn ErasedSource>> = vec![
            Box::new(workspace_source(&project_dir)),
            Box::new(environment_source(&project_dir)),
            Box::new(agents_md_source(&project_dir)),
            Box::new(instructions_source(&project_dir, &config.instructions)),
            Box::new(skill_guidance_source(guidance)),
        ];
        Self {
            project_dir,
            sources,
        }
    }

    pub fn agents_md_available(&self) -> bool {
        self.source_available("project/agents_md")
    }

    pub fn instructions_available(&self) -> bool {
        self.source_available("project/instructions")
    }

    /// At least one mandatory context source (AGENTS.md or instructions) is available.
    pub fn mandatory_context_satisfied(&self) -> bool {
        self.agents_md_available() || self.instructions_available()
    }

    pub fn can_initialize_baseline(&self) -> bool {
        self.mandatory_context_satisfied() && self.render_baseline().is_some()
    }

    fn source_available(&self, key: &str) -> bool {
        self.sources
            .iter()
            .find(|s| s.key_str() == key)
            .and_then(|s| s.baseline())
            .is_some()
    }

    pub fn render_baseline(&self) -> Option<Generation> {
        let mut parts = Vec::new();
        let mut snapshot = Snapshot::new();

        for source in &self.sources {
            if let Some((text, entry)) = source.baseline() {
                parts.push(text);
                snapshot.insert(source.key_str(), entry);
            }
        }

        if parts.is_empty() {
            return None;
        }

        Some(Generation {
            baseline: parts.join("\n\n"),
            snapshot,
        })
    }

    pub fn reconcile(&self, previous: &Snapshot) -> ReconcileResult {
        let mut combined_text = Vec::new();
        let mut snapshot = previous.clone();
        let mut changed = false;

        for source in &self.sources {
            match source.reconcile(&snapshot) {
                ReconcileResult::Unchanged => {}
                ReconcileResult::Updated { text, snapshot: s } => {
                    combined_text.push(text);
                    snapshot = s;
                    changed = true;
                }
                ReconcileResult::ReplacementBlocked => {
                    return ReconcileResult::ReplacementBlocked;
                }
                ReconcileResult::ReplacementReady { generation } => {
                    if baseline_differs_materially(&generation, previous) {
                        return ReconcileResult::ReplacementReady { generation };
                    }
                }
            }
        }

        if !self.mandatory_context_satisfied() {
            if changed {
                return ReconcileResult::Updated {
                    text: combined_text.join("\n\n"),
                    snapshot,
                };
            }
            return ReconcileResult::ReplacementBlocked;
        }

        if let Some(generation) = self.render_baseline() {
            if baseline_differs_materially(&generation, previous) {
                return ReconcileResult::ReplacementReady { generation };
            }
        }

        if changed {
            ReconcileResult::Updated {
                text: combined_text.join("\n\n"),
                snapshot,
            }
        } else {
            ReconcileResult::Unchanged
        }
    }

    pub async fn ensure_epoch(
        &self,
        store: &ContextEpochStore,
        sessions: &deepseekcode_storage::SessionStore,
        session_id: SessionId,
    ) -> Result<Option<Generation>> {
        if store.get(session_id).await?.is_some() {
            return Ok(None);
        }
        if !self.mandatory_context_satisfied() {
            return Ok(None);
        }
        let generation = self.render_baseline();
        if let Some(ref gen) = generation {
            let baseline_seq = sessions.latest_message_seq(session_id).await?;
            store.initialize(session_id, gen, baseline_seq).await?;
        }
        Ok(generation)
    }

    pub fn project_dir(&self) -> &str {
        &self.project_dir
    }
}

trait ErasedSource: Send + Sync {
    fn key_str(&self) -> String;
    fn baseline(&self) -> Option<(String, super::source::SourceSnapshot)>;
    fn reconcile(&self, previous: &Snapshot) -> ReconcileResult;
}

impl<A: Send + Sync + serde::de::DeserializeOwned + 'static> ErasedSource for ContextSource<A> {
    fn key_str(&self) -> String {
        self.key.0.clone()
    }

    fn baseline(&self) -> Option<(String, super::source::SourceSnapshot)> {
        match (self.load)() {
            LoadResult::Unavailable => None,
            LoadResult::Available(value) => {
                let text = (self.baseline)(&value);
                let encoded = (self.encode)(&value);
                Some((
                    text,
                    super::source::SourceSnapshot {
                        value: encoded,
                        removed: None,
                    },
                ))
            }
            LoadResult::Stale(value) => {
                let text = (self.baseline)(&value);
                let encoded = (self.encode)(&value);
                Some((
                    text,
                    super::source::SourceSnapshot {
                        value: encoded,
                        removed: Some("stale".into()),
                    },
                ))
            }
        }
    }

    fn reconcile(&self, previous: &Snapshot) -> ReconcileResult {
        match (self.load)() {
            LoadResult::Unavailable => {
                if let Some(prev) = previous.get(&self.key.0) {
                    let removed_text = prev.removed.clone().unwrap_or_else(|| {
                        self.removed
                            .map(|removed| {
                                if let Ok(decoded) = serde_json::from_value::<A>(prev.value.clone())
                                {
                                    removed(&decoded)
                                } else {
                                    format!("{} is no longer available.", self.key.0)
                                }
                            })
                            .unwrap_or_else(|| format!("{} is no longer available.", self.key.0))
                    });
                    reconcile_snapshot_removed(previous, &self.key, removed_text)
                } else {
                    ReconcileResult::Unchanged
                }
            }
            LoadResult::Available(value) | LoadResult::Stale(value) => {
                let key = &self.key;
                let encoded = (self.encode)(&value);
                let prev_entry = previous.get(&key.0);

                if let Some(prev) = prev_entry {
                    if prev.value == encoded {
                        return ReconcileResult::Unchanged;
                    }
                    let text =
                        if let Ok(prev_value) = serde_json::from_value::<A>(prev.value.clone()) {
                            (self.update)(&prev_value, &value)
                        } else {
                            (self.update)(&value, &value)
                        };
                    reconcile_snapshot(previous, key, encoded, text)
                } else {
                    let text = (self.baseline)(&value);
                    reconcile_snapshot(previous, key, encoded, text)
                }
            }
        }
    }
}

fn baseline_differs_materially(generation: &Generation, previous: &Snapshot) -> bool {
    generation
        .snapshot
        .keys()
        .any(|key| !previous.contains_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_storage::{Database, SessionStore};
    use deepseekcode_types::{LocationId, SessionId};
    use tempfile::TempDir;

    #[test]
    fn blocks_reconcile_when_mandatory_context_missing() {
        let dir = TempDir::new().unwrap();
        let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
        let baseline = registry.render_baseline().expect("environment baseline");
        let result = registry.reconcile(&baseline.snapshot);
        assert!(matches!(result, ReconcileResult::ReplacementBlocked));
    }

    #[test]
    fn replacement_ready_when_agents_md_newly_available() {
        let dir = TempDir::new().unwrap();
        let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
        let baseline = registry.render_baseline().expect("environment baseline");
        assert!(!baseline.snapshot.contains_key("project/agents_md"));

        std::fs::write(dir.path().join("AGENTS.md"), "# New Agent\nDo things.").unwrap();
        let result = registry.reconcile(&baseline.snapshot);
        match result {
            ReconcileResult::ReplacementReady { generation } => {
                assert!(generation.baseline.contains("New Agent"));
                assert!(generation.snapshot.contains_key("project/agents_md"));
            }
            other => panic!("expected ReplacementReady, got {other:?}"),
        }
    }

    #[test]
    fn blocks_when_agents_md_removed_without_fallback() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nRules.").unwrap();
        let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
        let baseline = registry.render_baseline().expect("baseline with agents_md");
        assert!(baseline.snapshot.contains_key("project/agents_md"));

        std::fs::remove_file(dir.path().join("AGENTS.md")).unwrap();
        let result = registry.reconcile(&baseline.snapshot);
        match result {
            ReconcileResult::Updated { text, .. } => {
                assert!(text.contains("no longer apply"));
            }
            other => panic!("expected Updated after agents_md removal, got {other:?}"),
        }
    }

    #[test]
    fn updated_when_agents_md_removed_with_instructions() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent\nRules.").unwrap();
        std::fs::write(dir.path().join("RULES.md"), "# Rules\nFollow these.").unwrap();
        let config = DeepSeekCodeConfig {
            instructions: vec!["RULES.md".into()],
            ..Default::default()
        };
        let registry = SystemContextRegistry::with_config(dir.path().to_string_lossy(), &config);
        let baseline = registry.render_baseline().expect("baseline with agents_md");
        assert!(baseline.snapshot.contains_key("project/agents_md"));

        std::fs::remove_file(dir.path().join("AGENTS.md")).unwrap();
        let result = registry.reconcile(&baseline.snapshot);
        match result {
            ReconcileResult::Updated { text, snapshot } => {
                assert!(text.contains("no longer apply"));
                assert!(!snapshot.contains_key("project/agents_md"));
                assert!(snapshot.contains_key("project/instructions"));
            }
            other => panic!("expected Updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn initializes_context_epoch_from_agents_md() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Test Agent\nDo things.").unwrap();

        let registry = SystemContextRegistry::new(dir.path().to_string_lossy());
        let baseline = registry.render_baseline().expect("baseline");
        assert!(baseline.baseline.contains("Test Agent"));

        let db = Database::open_in_memory().await.unwrap();
        let sessions = SessionStore::new(db.pool().clone());
        let session_id = SessionId::new();
        sessions
            .create(
                session_id,
                LocationId::new(),
                &dir.path().to_string_lossy(),
                deepseekcode_schema::agent::AgentKind::Build,
                None,
                None,
            )
            .await
            .unwrap();

        let store = ContextEpochStore::new(db.pool().clone());
        registry
            .ensure_epoch(&store, &sessions, session_id)
            .await
            .unwrap();
        assert!(store.get(session_id).await.unwrap().is_some());
    }
}
