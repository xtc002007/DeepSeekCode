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

/// Sync instruction-URL fetch used by the (sync) context loaders.
///
/// Lock-order / runtime contract: loaders run inside async runner tasks on
/// either runtime flavor. `Handle::block_on` on a `current_thread` runtime
/// from within an async context panics ("Cannot block the current thread"),
/// so that path is eliminated: `current_thread` always offloads to a
/// dedicated OS thread with its own runtime (never `h.block_on`).
/// `MultiThread` may use `block_in_place`; no runtime at all uses a fresh
/// one directly. Results are cached 5min so repeated `drain` turns do not
/// refetch; call [`prefetch_instruction_urls`] at startup/reload to warm the
/// cache asynchronously (loaders then read cache only on hit).
pub(crate) fn blocking_http_get(url: &str) -> Result<String, String> {
    if let Some(cached) = instruction_url_cache_get(url) {
        return Ok(cached);
    }
    let fetched = blocking_http_get_uncached(url)?;
    instruction_url_cache_insert(url, fetched.clone());
    Ok(fetched)
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
}

async fn http_get_async(url: &str) -> Result<String, String> {
    let client = build_http_client()?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.text().await.map_err(|e| e.to_string())
}

fn blocking_http_get_uncached(url: &str) -> Result<String, String> {
    let rt_handle = tokio::runtime::Handle::try_current().ok();
    match rt_handle {
        Some(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| h.block_on(http_get_async(url)))
        }
        Some(_) => {
            // current_thread runtime: NEVER `h.block_on` here (panics when
            // called from async context). Run the fetch on a dedicated OS
            // thread with its own current-thread runtime.
            let url_owned = url.to_string();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                rt.block_on(http_get_async(&url_owned))
            })
            .join()
            .map_err(|_| "instruction URL fetch thread panicked".to_string())?
        }
        None => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(http_get_async(url))
        }
    }
}

type UrlCache = std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String)>>;

fn instruction_url_cache() -> &'static UrlCache {
    static CACHE: std::sync::OnceLock<UrlCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn instruction_url_cache_get(url: &str) -> Option<String> {
    let guard = instruction_url_cache().lock().ok()?;
    let (at, body) = guard.get(url)?;
    if at.elapsed() > std::time::Duration::from_secs(300) {
        return None;
    }
    Some(body.clone())
}

fn instruction_url_cache_insert(url: &str, body: String) {
    if let Ok(mut guard) = instruction_url_cache().lock() {
        guard.insert(url.to_string(), (std::time::Instant::now(), body));
    }
}

/// Async prefetch for startup/reload paths: warms the URL cache without
/// blocking the executor. Sync loaders then hit the cache.
pub async fn prefetch_instruction_urls(urls: &[String]) {
    for url in urls {
        if instruction_url_cache_get(url).is_some() {
            continue;
        }
        if let Ok(body) = http_get_async(url).await {
            const MAX_REMOTE_CHARS: usize = 8_000;
            let bounded: String = body.chars().take(MAX_REMOTE_CHARS).collect();
            if !bounded.is_empty() {
                instruction_url_cache_insert(url, bounded);
            }
        }
    }
}

fn load_instruction_url(url: &str, texts: &mut Vec<String>) {
    // Issue-8: 5s timeout + failure preserves old prefix. A hanging or
    // flapping URL must never block `drain` nor rewrite the system prefix
    // every turn (each rewrite = full DeepSeek miss).
    tracing::debug!(
        target: code_plus_plus_telemetry::APP,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        url = %url,
        "instruction URL fetch started"
    );
    let Ok(content) = blocking_http_get(url) else {
        tracing::warn!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            url = %url,
            "instruction URL fetch failed; keeping cached prefix"
        );
        return;
    };
    // Bound single remote doc so one huge wiki cannot blow the prefix.
    const MAX_REMOTE_CHARS: usize = 8_000;
    let bounded: String = content.chars().take(MAX_REMOTE_CHARS).collect();
    if !bounded.is_empty() {
        texts.push(bounded);
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
            if let Some(global) = code_plus_plus_types::config_dir() {
                let global_agents = global.join("code-plus-plus").join("AGENTS.md");
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

/// Issue-9: standing memory (`.code-plus-plus/MEMORY.md`). New file →
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
