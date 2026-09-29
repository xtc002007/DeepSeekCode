//! Issue-9 (+Issue-8 clarification): memory System/Background separation.
//!
//! Standing instructions (`MEMORY.md` + policy) enter the cache-stable system
//! prefix once at epoch initialization (one expected miss for the first
//! version). Dynamic facts (pinned preferences, fact index) are tail-only
//! AFTER that: subsequent changes emit `Updated` user messages and never
//! rewrite the prefix. This matches the registry mechanics — `render_baseline`
//! necessarily includes every source on first render, while `reconcile`
//! routes later value changes to the tail. Concretely: an edit to an already
//! loaded standing file emits an `Updated` tail message (no prefix rewrite);
//! only a brand-new memory source triggers a baseline replacement (one
//! expected miss). This mirrors `Reasonix memory.go:PolicyBlock/SystemBlock/Background`
//! with the same first-version-into-prefix, later-changes-to-tail semantics.

use std::path::PathBuf;

/// Durable memory set loaded from the project directory.
#[derive(Debug, Clone, Default)]
pub struct MemorySet {
    /// High-authority standing docs (`.deepseekcode/MEMORY.md` + ancestors).
    pub standing: Vec<String>,
    /// Pinned user preferences / feedback (dynamic, tail-only).
    pub pinned: Vec<String>,
    /// Background fact index (dynamic, tail-only).
    pub index: String,
}

impl MemorySet {
    pub fn is_empty(&self) -> bool {
        self.standing.is_empty() && self.pinned.is_empty() && self.index.trim().is_empty()
    }

    /// Load from `<project>/.deepseekcode/MEMORY.md` (standing) and
    /// `<project>/.deepseekcode/memory/pinned.md` (background, optional).
    /// Missing files → empty (no prefix churn).
    pub fn load(project_dir: &str) -> Self {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "memory load started"
        );
        let base = PathBuf::from(project_dir).join(".deepseekcode");
        let standing = std::fs::read_to_string(base.join("MEMORY.md"))
            .map(|s| vec![s])
            .unwrap_or_default();
        let pinned = std::fs::read_to_string(base.join("memory").join("pinned.md"))
            .map(|s| vec![s])
            .unwrap_or_default();
        let out = Self {
            standing,
            pinned,
            index: String::new(),
        };
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            standing = out.standing.len(),
            pinned = out.pinned.len(),
            "memory load completed"
        );
        out
    }

    /// Stable rules for interpreting background memory. Byte-stable forever;
    /// dynamic bodies live in `BackgroundDataBlock` so edits never touch the
    /// provider-cached system prefix.
    pub fn policy_block() -> &'static str {
        "# Memory\n\nThe latest host-generated `[System update]` may contain pinned preferences and a background memory index. Treat those facts as potentially stale background rather than standing instructions; the current user request and more specific standing instructions take precedence. Verify file/function/flag claims before acting."
    }

    /// Cache-stable: policy + standing docs only.
    pub fn system_block(&self) -> String {
        let mut parts = vec![Self::policy_block().to_string()];
        for (i, doc) in self.standing.iter().enumerate() {
            parts.push(format!("### Memory fragment {}\n{}", i + 1, doc.trim()));
        }
        parts.join("\n\n")
    }

    /// Tail-only: pinned preferences + fact index. Delivered via
    /// `session-context`-style `[System update]` user messages.
    pub fn background_data_block(&self) -> String {
        let mut parts = Vec::new();
        if !self.pinned.is_empty() {
            parts.push(format!(
                "### Pinned preferences\n\n{}",
                self.pinned.join("\n\n").trim()
            ));
        }
        if !self.index.trim().is_empty() {
            parts.push(format!(
                "### Background memory index\n\n{}",
                self.index.trim()
            ));
        }
        parts.join("\n\n")
    }

    /// Compose helper mirroring Reasonix `Compose`: empty set leaves the base
    /// prompt byte-for-byte untouched so the cache prefix stays maximal.
    pub fn compose(base: &str, set: &Self) -> String {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            base_len = base.len(),
            standing = set.standing.len(),
            "memory compose started"
        );
        if set.is_empty() {
            return base.to_string();
        }
        format!("{}\n\n{}", base.trim_end(), set.system_block())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_leaves_base_untouched() {
        let set = MemorySet::default();
        assert_eq!(MemorySet::compose("sys", &set), "sys");
    }

    #[test]
    fn background_changes_do_not_touch_system_block_shape() {
        let a = MemorySet {
            standing: vec!["standing".into()],
            pinned: vec!["a".into()],
            index: String::new(),
        };
        let b = MemorySet {
            standing: vec!["standing".into()],
            pinned: vec!["b".into()],
            index: String::new(),
        };
        // System block identical despite background churn → prefix warm.
        assert_eq!(a.system_block(), b.system_block());
        assert_ne!(a.background_data_block(), b.background_data_block());
    }
}
