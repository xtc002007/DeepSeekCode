use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContextKey(pub String);

impl ContextKey {
    pub fn new(namespace: &str, name: &str) -> Self {
        Self(format!("{namespace}/{name}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub value: serde_json::Value,
    pub removed: Option<String>,
}

pub type Snapshot = BTreeMap<String, SourceSnapshot>;

#[derive(Debug, Clone)]
pub struct Generation {
    pub baseline: String,
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone)]
pub enum ReconcileResult {
    Unchanged,
    Updated { text: String, snapshot: Snapshot },
    ReplacementReady { generation: Generation },
    ReplacementBlocked,
}

pub enum LoadResult<A> {
    Available(A),
    Unavailable,
    Stale(A),
}

pub struct ContextSource<A> {
    pub key: ContextKey,
    pub load: Box<dyn Fn() -> LoadResult<A> + Send + Sync>,
    pub baseline: fn(&A) -> String,
    pub update: fn(&A, &A) -> String,
    pub removed: Option<fn(&A) -> String>,
    pub encode: fn(&A) -> serde_json::Value,
}

impl<A: 'static> ContextSource<A> {
    pub fn make(
        key: ContextKey,
        load: impl Fn() -> LoadResult<A> + Send + Sync + 'static,
        baseline: fn(&A) -> String,
        update: fn(&A, &A) -> String,
        encode: fn(&A) -> serde_json::Value,
    ) -> Self {
        Self {
            key,
            load: Box::new(load),
            baseline,
            update,
            removed: None,
            encode,
        }
    }

    pub fn with_removed(mut self, removed: fn(&A) -> String) -> Self {
        self.removed = Some(removed);
        self
    }
}

pub fn agents_md_source(project_dir: &str) -> ContextSource<Vec<String>> {
    let dir = project_dir.to_string();
    ContextSource::make(
        ContextKey::new("project", "agents_md"),
        move || {
            let mut files = Vec::new();
            let mut current = Some(std::path::PathBuf::from(&dir));
            while let Some(path) = current {
                let agents = path.join("AGENTS.md");
                if agents.exists() {
                    if let Ok(content) = std::fs::read_to_string(&agents) {
                        files.push(content);
                    }
                }
                current = path.parent().map(|p| p.to_path_buf());
            }
            if files.is_empty() {
                LoadResult::Unavailable
            } else {
                LoadResult::Available(files)
            }
        },
        |files| {
            files
                .iter()
                .enumerate()
                .map(|(i, f)| format!("### AGENTS.md fragment {}\n{f}", i + 1))
                .collect::<Vec<_>>()
                .join("\n\n")
        },
        |_prev, curr| {
            format!(
                "Project instructions updated.\n\n{}",
                curr.iter()
                    .enumerate()
                    .map(|(i, f)| format!("### AGENTS.md fragment {}\n{f}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            )
        },
        |files| serde_json::to_value(files).unwrap_or(serde_json::Value::Null),
    )
    .with_removed(|_| "Previously loaded AGENTS.md instructions no longer apply.".into())
}

fn is_instruction_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn is_instruction_glob(source: &str) -> bool {
    source.contains('*') || source.contains('?') || source.contains('[')
}

fn load_instruction_file(path: &std::path::Path, texts: &mut Vec<String>) {
    if path.is_file() {
        if let Ok(content) = std::fs::read_to_string(path) {
            texts.push(content);
        }
    }
}

fn load_instruction_glob(project_dir: &str, pattern: &str, texts: &mut Vec<String>) {
    let base = if std::path::Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        std::path::PathBuf::from(project_dir)
            .join(pattern)
            .to_string_lossy()
            .to_string()
    };
    if let Ok(entries) = glob::glob(&base) {
        let mut paths: Vec<_> = entries.flatten().collect();
        paths.sort();
        for path in paths {
            load_instruction_file(&path, texts);
        }
    }
}

/// Sync blocking GET that never parks the async executor. Source loaders run
/// inside async runner tasks; a 5s remote hang on an executor thread would
/// stall turns. Outside a multi-thread runtime (tests/CLI bootstrap) it runs
/// inline. Shared by instruction URLs and the skills index so both stay
/// bounded the same way.
pub(crate) fn blocking_http_get(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<reqwest::blocking::Response, reqwest::Error> {
    let in_multi_thread = tokio::runtime::Handle::try_current()
        .map(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        .unwrap_or(false);
    if in_multi_thread {
        tokio::task::block_in_place(|| client.get(url).send())
    } else {
        client.get(url).send()
    }
}

fn load_instruction_url(url: &str, texts: &mut Vec<String>) {
    // Issue-8: 5s timeout + failure preserves old prefix. A hanging or
    // flapping URL must never block `drain` nor rewrite the system prefix
    // every turn (each rewrite = full DeepSeek miss).
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let Ok(client) = client else { return };
    let Ok(response) = blocking_http_get(&client, url) else {
        tracing::warn!(
            target: deepseekcode_telemetry::APP,
            url = %url,
            "instruction URL fetch failed; keeping cached prefix"
        );
        return;
    };
    if response.status().is_success() {
        if let Ok(content) = response.text() {
            // Bound single remote doc so one huge wiki cannot blow the prefix.
            const MAX_REMOTE_CHARS: usize = 8_000;
            let bounded: String = content.chars().take(MAX_REMOTE_CHARS).collect();
            if !bounded.is_empty() {
                texts.push(bounded);
            }
        }
    }
}

/// P2-7: standing-source keys whose content edits may replace the baseline
/// when `context.standing_replacement_on_edit` is on. Background, workspace,
/// environment, and skill catalog sources are never standing: their churn
/// must always ride the tail.
pub fn is_standing_source_key(key: &str) -> bool {
    matches!(
        key,
        "memory/standing" | "project/agents_md" | "project/instructions"
    )
}

pub fn workspace_source(project_dir: &str) -> ContextSource<serde_json::Value> {
    let dir = project_dir.to_string();
    ContextSource::make(
        ContextKey::new("project", "workspace"),
        move || {
            LoadResult::Available(serde_json::json!({
                "workspace_root": dir,
                "shell": if cfg!(windows) { "cmd.exe" } else { "bash" },
            }))
        },
        |info| {
            let root = info["workspace_root"].as_str().unwrap_or("");
            let shell = info["shell"].as_str().unwrap_or("shell");
            format!(
                "Workspace:\n- Root: {root}\n- Shell tool uses: {shell}\n\
                 - Analyze and modify files only under this root using relative paths unless the user asks otherwise."
            )
        },
        |_, curr| {
            let root = curr["workspace_root"].as_str().unwrap_or("");
            format!("Workspace root changed to: {root}")
        },
        |info| info.clone(),
    )
}

pub fn environment_source(project_dir: &str) -> ContextSource<serde_json::Value> {
    let dir = project_dir.to_string();
    ContextSource::make(
        ContextKey::new("host", "environment"),
        move || {
            LoadResult::Available(serde_json::json!({
                "cwd": dir,
                "workspace_root": dir,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            }))
        },
        |env| {
            let root = env["workspace_root"].as_str().unwrap_or("");
            let os = env["os"].as_str().unwrap_or("");
            let arch = env["arch"].as_str().unwrap_or("");
            format!(
                "Environment:\n- Workspace root: {root}\n- OS: {os}\n- Arch: {arch}\n\
                 - Use paths relative to the workspace root (e.g. `src/main.rs`), not absolute paths to other directories."
            )
        },
        |_, curr| {
            let root = curr["workspace_root"].as_str().unwrap_or("");
            format!("Workspace root changed to: {root}")
        },
        |env| env.clone(),
    )
}

pub fn instructions_source(
    project_dir: &str,
    instruction_paths: &[String],
) -> ContextSource<Vec<String>> {
    let dir = project_dir.to_string();
    let paths = instruction_paths.to_vec();
    ContextSource::make(
        ContextKey::new("project", "instructions"),
        move || {
            let mut texts = Vec::new();
            for source in &paths {
                if is_instruction_url(source) {
                    load_instruction_url(source, &mut texts);
                } else if is_instruction_glob(source) {
                    load_instruction_glob(&dir, source, &mut texts);
                } else {
                    let base = if std::path::Path::new(source).is_absolute() {
                        std::path::PathBuf::from(source)
                    } else {
                        std::path::PathBuf::from(&dir).join(source)
                    };
                    load_instruction_file(&base, &mut texts);
                }
            }
            if let Some(global) = dirs::config_dir() {
                let global_agents = global.join("deepseekcode").join("AGENTS.md");
                if global_agents.exists() {
                    if let Ok(content) = std::fs::read_to_string(&global_agents) {
                        texts.push(content);
                    }
                }
            }
            if texts.is_empty() {
                LoadResult::Unavailable
            } else {
                LoadResult::Available(texts)
            }
        },
        |texts| {
            texts
                .iter()
                .enumerate()
                .map(|(i, t)| format!("### Instruction fragment {}\n{t}", i + 1))
                .collect::<Vec<_>>()
                .join("\n\n")
        },
        |_, curr| {
            format!(
                "Instructions updated.\n\n{}",
                curr.iter()
                    .enumerate()
                    .map(|(i, t)| format!("### Instruction fragment {}\n{t}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            )
        },
        |texts| serde_json::to_value(texts).unwrap_or(serde_json::Value::Null),
    )
    .with_removed(|_| "Project instructions were removed.".into())
}

/// Issue-9: standing memory (`.deepseekcode/MEMORY.md`). New file →
/// `ReplacementReady` (one expected miss, then stable); edits fold on next
/// epoch. Never carries dynamic facts.
pub fn memory_standing_source(project_dir: &str) -> ContextSource<Vec<String>> {
    let dir = project_dir.to_string();
    ContextSource::make(
        ContextKey::new("memory", "standing"),
        move || {
            let set = crate::memory::MemorySet::load(&dir);
            if set.standing.is_empty() {
                LoadResult::Unavailable
            } else {
                LoadResult::Available(set.standing)
            }
        },
        |docs| {
            docs.iter()
                .enumerate()
                .map(|(i, d)| format!("### Memory fragment {}\n{}", i + 1, d.trim()))
                .collect::<Vec<_>>()
                .join("\n\n")
        },
        |_, curr| {
            format!(
                "Memory updated.\n\n{}",
                curr.iter()
                    .enumerate()
                    .map(|(i, d)| format!("### Memory fragment {}\n{}", i + 1, d.trim()))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            )
        },
        |docs| serde_json::to_value(docs).unwrap_or(serde_json::Value::Null),
    )
    .with_removed(|_| "Previously loaded memory instructions no longer apply.".into())
}

/// Issue-9 (+Issue-8 clarification): background memory (pinned facts). The
/// FIRST version necessarily enters the baseline (all sources render into it;
/// one expected miss), but every LATER change emits `Updated` tail user
/// messages and never rewrites the system prefix.
pub fn memory_background_source(project_dir: &str) -> ContextSource<String> {
    let dir = project_dir.to_string();
    ContextSource::make(
        ContextKey::new("memory", "background"),
        move || {
            let set = crate::memory::MemorySet::load(&dir);
            let block = set.background_data_block();
            if block.trim().is_empty() {
                LoadResult::Unavailable
            } else {
                LoadResult::Available(block)
            }
        },
        |block| block.clone(),
        |_, curr| curr.clone(),
        |block| serde_json::to_value(block).unwrap_or(serde_json::Value::Null),
    )
    .with_removed(|_| "Pinned memory facts were removed.".into())
}

pub fn skill_guidance_source(guidance_text: String) -> ContextSource<String> {
    let text = guidance_text;
    ContextSource::make(
        ContextKey::new("core", "skill_guidance"),
        move || {
            if text.is_empty() {
                LoadResult::Unavailable
            } else {
                LoadResult::Available(text.clone())
            }
        },
        |t| t.clone(),
        |_, curr| format!("Skill guidance updated.\n\n{curr}"),
        |t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null),
    )
}
