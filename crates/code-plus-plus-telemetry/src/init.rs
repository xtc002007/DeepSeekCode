use crate::context::RunContext;
use crate::export::prune_old_runs;
use crate::filter::{build_filter_string, resolve_console_enabled, resolve_logs_root};
use code_plus_plus_config::{LogFormat, LoggingConfig};
use code_plus_plus_types::{CodePlusPlusError, Result};
use serde_json::json;
use std::fs;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

static INIT_LOCK: Mutex<()> = Mutex::new(());
static GUARDS: Mutex<Vec<tracing_appender::non_blocking::WorkerGuard>> = Mutex::new(Vec::new());

/// Initialize global tracing. Idempotent — returns existing context on repeat calls.
/// Retryable: if the first attempt fails (e.g. logs dir unwritable), the next
/// call retries instead of permanently falling back to `"unknown"`.
pub fn init(component: &str, config: &LoggingConfig) -> RunContext {
    if let Some(ctx) = RunContext::global() {
        return ctx.clone();
    }

    // Serialize concurrent first-init; double-check after acquiring the lock.
    // `Mutex::lock` may poison on panic — treat as retryable, not fatal.
    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ctx) = RunContext::global() {
        return ctx.clone();
    }

    match init_inner(component, config) {
        Ok(ctx) => {
            RunContext::set_global(ctx.clone());
            ctx
        }
        Err(e) => {
            // Never swallow: surface to stderr so a broken log dir is visible
            // even though we must still return a usable (fallback) context.
            eprintln!("code-plus-plus telemetry init failed: {e}; retry on next init");
            fallback_context(component)
        }
    }
}

/// Returns the current run id for structured log correlation.
/// Falls back to `"unknown"` when telemetry was never initialized so call
/// sites can always emit `run_id = %code_plus_plus_telemetry::current_run_id()`.
///
/// Hot-path cache: `run_id` is immutable after first successful `init`, so
/// each thread caches the first non-`"unknown"` value and skips the global
/// lookup + `String` clone afterwards. Pre-init `"unknown"` is never cached,
/// so a later `init` is picked up on the next call.
pub fn current_run_id() -> String {
    std::thread_local! {
        static CACHED_RUN_ID: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(cached) = CACHED_RUN_ID
        .try_with(|c| c.borrow().clone())
        .unwrap_or(None)
    {
        return cached;
    }
    let id = RunContext::global()
        .map(|c| c.run_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    // Only cache real ids; `"unknown"` must re-check so post-init calls heal.
    if id != "unknown" {
        let _ = CACHED_RUN_ID.try_with(|c| *c.borrow_mut() = Some(id.clone()));
    }
    id
}

/// Ensure telemetry is initialized, loading `LoggingConfig` from
/// `project_dir` on first use. Safe to call from SDK / embed / server paths
/// that may run without going through CLI/Desktop startup. Idempotent.
pub fn ensure_init(component: &str, project_dir: Option<&str>) -> RunContext {
    if let Some(ctx) = RunContext::global() {
        return ctx.clone();
    }
    let dir = project_dir.unwrap_or(".");
    let (config, config_error) =
        match code_plus_plus_config::load_effective_config(camino::Utf8Path::new(dir)) {
            Ok(c) => (c.logging, None),
            Err(e) => {
                let msg = format!("config load failed for '{dir}' ({e}); using default logging");
                eprintln!("code-plus-plus: {msg}");
                (Default::default(), Some(msg))
            }
        };
    let ctx = init(component, &config);
    // Stage two: subscriber now exists, persist the same warning to app.log.
    if let Some(err) = config_error.as_deref() {
        tracing::warn!(
            target: crate::targets::APP,
            run_id = %ctx.run_id,
            error = %err,
            "config load failed; using default logging"
        );
    }
    ctx
}

fn init_inner(component: &str, config: &LoggingConfig) -> Result<RunContext> {
    let logs_root = resolve_logs_root(config);
    fs::create_dir_all(&logs_root).map_err(CodePlusPlusError::Io)?;

    let run_id = ulid::Ulid::new().to_string();
    let run_dir = logs_root.join(format!("run-{run_id}"));
    fs::create_dir_all(&run_dir).map_err(CodePlusPlusError::Io)?;
    fs::write(logs_root.join("latest_run.txt"), &run_id).map_err(CodePlusPlusError::Io)?;
    prune_old_runs(&logs_root, config.max_run_dirs)?;

    let ctx = RunContext {
        run_id: run_id.clone(),
        component: component.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        run_dir: run_dir.clone(),
        logs_root,
    };

    fs::write(
        run_dir.join("meta.json"),
        serde_json::to_string_pretty(&json!({
            "run_id": run_id,
            "component": component,
            "app_version": ctx.app_version,
            "pid": ctx.pid,
            "platform": ctx.platform,
            "arch": ctx.arch,
        }))
        .unwrap_or_default(),
    )
    .map_err(CodePlusPlusError::Io)?;

    let filter = EnvFilter::try_new(build_filter_string(config))
        .unwrap_or_else(|_| EnvFilter::new("code-plus-plus=info"));

    let console = resolve_console_enabled(config);
    let file = config.file;
    let audit = config.audit.enabled;
    let pretty = matches!(config.format, LogFormat::Pretty);
    // Product-grade fix: `max_file_mb` is owned by `LoggingConfig`, never
    // re-loaded from `"."` (previously `open_log` ignored the caller's
    // `--directory` and always probed `.`, so per-project caps were lost).
    let max_bytes = config.max_file_mb.clamp(1, 1024) * 1024 * 1024;

    if file {
        let (app_w, g1) = open_log(&run_dir, "app.log", max_bytes)?;
        let (sess_w, g2) = open_log(&run_dir, "session.log", max_bytes)?;
        push_guard(g1);
        push_guard(g2);
        // `session.log` carries session + tool + llm so a full turn can be
        // reconstructed from one file. `app.log` remains the catch-all.
        let session_filter = tracing_subscriber::filter::filter_fn(|m| {
            crate::targets::is_session_log_target(m.target())
        });

        if audit {
            let (audit_w, g3) = open_log(&run_dir, "audit.log", max_bytes)?;
            push_guard(g3);
            let audit_filter = tracing_subscriber::filter::filter_fn(|m| {
                crate::targets::is_audit_target(m.target())
            });
            if console && pretty {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(sess_w)
                            .with_filter(session_filter),
                    )
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(audit_w)
                            .with_filter(audit_filter),
                    )
                    .with(fmt::layer().pretty().with_writer(io::stdout))
                    .try_init()
                    .ok();
            } else if console {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(sess_w)
                            .with_filter(session_filter),
                    )
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(audit_w)
                            .with_filter(audit_filter),
                    )
                    .with(fmt::layer().json().with_writer(io::stdout))
                    .try_init()
                    .ok();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(sess_w)
                            .with_filter(session_filter),
                    )
                    .with(
                        fmt::layer()
                            .json()
                            .with_writer(audit_w)
                            .with_filter(audit_filter),
                    )
                    .try_init()
                    .ok();
            }
        } else if console && pretty {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(
                    fmt::layer()
                        .json()
                        .with_writer(sess_w)
                        .with_filter(session_filter),
                )
                .with(fmt::layer().pretty().with_writer(io::stdout))
                .try_init()
                .ok();
        } else if console {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(
                    fmt::layer()
                        .json()
                        .with_writer(sess_w)
                        .with_filter(session_filter),
                )
                .with(fmt::layer().json().with_writer(io::stdout))
                .try_init()
                .ok();
        } else {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(
                    fmt::layer()
                        .json()
                        .with_writer(sess_w)
                        .with_filter(session_filter),
                )
                .try_init()
                .ok();
        }
    } else if console && pretty {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty().with_writer(io::stdout))
            .try_init()
            .ok();
    } else if console {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_writer(io::stdout))
            .try_init()
            .ok();
    } else {
        tracing_subscriber::registry().with(filter).try_init().ok();
    }

    install_panic_hook(&run_dir, &run_id);
    tracing::info!(
        target: crate::targets::APP,
        run_id = %run_id,
        component = component,
        run_dir = %run_dir.display(),
        "telemetry initialized"
    );

    Ok(ctx)
}

fn open_log(
    run_dir: &Path,
    name: &str,
    max_bytes: u64,
) -> Result<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    // Product-grade fix: true per-write size rotation. Previously only a
    // startup rename (`app.log` -> `app.log.1`) existed, so a long run grew
    // `app.log` unbounded. `SizeRotatingWriter` checks every write, closes
    // the handle before rename (Windows-safe), and keeps `name` + `name.1`.
    let writer =
        SizeRotatingWriter::open(run_dir, name, max_bytes).map_err(CodePlusPlusError::Io)?;
    Ok(tracing_appender::non_blocking(writer))
}

/// Append-only writer with per-write size cap.
///
/// Keeps at most five files: `name` plus `name.1` … `name.4` (newest roll is
/// `.1`). When the next write would exceed `max_bytes`, the current file is
/// closed, older rolls shift down (`.3`→`.4`, …, `name`→`.1`, dropping `.4`),
/// and a fresh `name` is opened. Close-before-rename keeps Windows happy
/// (rename-while-open fails with sharing violation).
struct SizeRotatingWriter {
    dir: PathBuf,
    name: String,
    max_bytes: u64,
    state: std::sync::Mutex<WriterState>,
}

struct WriterState {
    file: Option<std::fs::File>,
    size: u64,
}

impl SizeRotatingWriter {
    fn open(dir: &Path, name: &str, max_bytes: u64) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(name);
        // Startup roll preserves the previous invariant: never start a run
        // with an over-cap file in place.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > max_bytes {
                Self::roll_files_static(dir, name);
            }
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            dir: dir.to_path_buf(),
            name: name.to_string(),
            max_bytes: max_bytes.max(64 * 1024),
            state: std::sync::Mutex::new(WriterState {
                file: Some(file),
                size,
            }),
        })
    }

    fn roll_files_static(dir: &Path, name: &str) {
        // Shift `.3`→`.4`, `.2`→`.3`, `.1`→`.2`, `name`→`.1`; drop old `.4`.
        // Each rename is best-effort so a half-rolled set never wedges logging.
        const KEEP: usize = 4;
        let oldest = dir.join(format!("{name}.{KEEP}"));
        let _ = std::fs::remove_file(&oldest);
        for i in (1..KEEP).rev() {
            let src = dir.join(format!("{name}.{i}"));
            let dst = dir.join(format!("{name}.{}", i + 1));
            if src.exists() {
                let _ = std::fs::rename(&src, &dst);
            }
        }
        let path = dir.join(name);
        let rolled = dir.join(format!("{name}.1"));
        if path.exists() {
            let _ = std::fs::rename(&path, &rolled);
        }
    }

    fn rotate_locked(&self, state: &mut WriterState) {
        if let Some(mut f) = state.file.take() {
            let _ = f.flush();
            drop(f);
        }
        Self::roll_files_static(&self.dir, &self.name);
        let path = self.dir.join(&self.name);
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            Ok(f) => {
                state.file = Some(f);
                state.size = 0;
            }
            Err(_) => {
                // Best-effort reopen in append mode so logging never wedges.
                if let Ok(f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                    state.file = Some(f);
                    state.size = size;
                }
            }
        }
    }
}

impl std::io::Write for SizeRotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // `non_blocking` calls from a single worker thread, but `Mutex`
        // keeps direct writes safe too.
        let mut state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("log writer lock poisoned"))?;
        if !buf.is_empty() && state.size.saturating_add(buf.len() as u64) > self.max_bytes {
            // Avoid infinite rotation on a single giant write: rotate once,
            // then accept the write even if it alone exceeds the cap.
            if state.size > 0 {
                self.rotate_locked(&mut state);
            }
        }
        let Some(file) = state.file.as_mut() else {
            return Err(std::io::Error::other("log file closed"));
        };
        let n = file.write(buf)?;
        state.size = state.size.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("log writer lock poisoned"))?;
        if let Some(file) = state.file.as_mut() {
            file.flush()
        } else {
            Ok(())
        }
    }
}

fn push_guard(guard: tracing_appender::non_blocking::WorkerGuard) {
    if let Ok(mut guards) = GUARDS.lock() {
        guards.push(guard);
    }
}

fn install_panic_hook(run_dir: &Path, run_id: &str) {
    let dir = run_dir.to_path_buf();
    let run_id = run_id.to_string();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("panic: {info}");
        tracing::error!(
            target: crate::targets::APP,
            run_id = %run_id,
            "{msg}"
        );
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("panic.log"))
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "{msg}")
            });
        eprintln!("{msg}");
    }));
}

fn fallback_context(component: &str) -> RunContext {
    RunContext {
        run_id: "unknown".into(),
        component: component.to_string(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        run_dir: PathBuf::from("."),
        logs_root: PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context(run_id: &str) -> RunContext {
        RunContext {
            run_id: run_id.into(),
            component: "test".into(),
            app_version: "0.0.0".into(),
            pid: 0,
            platform: "test".into(),
            arch: "test".into(),
            run_dir: PathBuf::from("."),
            logs_root: PathBuf::from("."),
        }
    }

    #[test]
    fn current_run_id_is_stable_and_matches_global() {
        // Repeated calls agree, and agree with the process global when set.
        // (`"unknown"` is never thread-cached, so pre/post-init agree too.)
        let a = current_run_id();
        let b = current_run_id();
        assert_eq!(a, b);
        if let Some(g) = RunContext::global() {
            assert_eq!(a, g.run_id);
        }
        // A fresh thread (fresh thread-local cache) observes the same value.
        let c = std::thread::spawn(current_run_id)
            .join()
            .expect("worker thread");
        assert_eq!(a, c);
    }

    #[test]
    fn unknown_heals_once_initialized() {
        // Mirrors production one-time init: the process global starts unset
        // in unit tests (no test calls `init()`); first set wins.
        if RunContext::global().is_none() {
            RunContext::set_global(test_context("test-run-id"));
        }
        let g = RunContext::global().expect("global set above");
        // Same thread re-reads the global even if it previously observed
        // `"unknown"` (unknown values are never cached).
        assert_eq!(current_run_id(), g.run_id);
        let c = std::thread::spawn(current_run_id)
            .join()
            .expect("worker thread");
        assert_eq!(c, g.run_id);
    }
}
