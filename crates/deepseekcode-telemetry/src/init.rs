use crate::context::RunContext;
use crate::export::prune_old_runs;
use crate::filter::{build_filter_string, resolve_console_enabled, resolve_logs_root};
use deepseekcode_config::{LogFormat, LoggingConfig};
use deepseekcode_types::{DeepSeekCodeError, Result};
use serde_json::json;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

static INIT: std::sync::Once = std::sync::Once::new();
static GUARDS: Mutex<Vec<tracing_appender::non_blocking::WorkerGuard>> = Mutex::new(Vec::new());

/// Initialize global tracing. Idempotent — returns existing context on repeat calls.
pub fn init(component: &str, config: &LoggingConfig) -> RunContext {
    if let Some(ctx) = RunContext::global() {
        return ctx.clone();
    }

    INIT.call_once(|| {
        if let Ok(ctx) = init_inner(component, config) {
            RunContext::set_global(ctx);
        }
    });

    RunContext::global()
        .cloned()
        .unwrap_or_else(|| fallback_context(component))
}

fn init_inner(component: &str, config: &LoggingConfig) -> Result<RunContext> {
    let logs_root = resolve_logs_root(config);
    fs::create_dir_all(&logs_root).map_err(DeepSeekCodeError::Io)?;

    let run_id = ulid::Ulid::new().to_string();
    let run_dir = logs_root.join(format!("run-{run_id}"));
    fs::create_dir_all(&run_dir).map_err(DeepSeekCodeError::Io)?;
    fs::write(logs_root.join("latest_run.txt"), &run_id).map_err(DeepSeekCodeError::Io)?;
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
    .map_err(DeepSeekCodeError::Io)?;

    let filter = EnvFilter::try_new(build_filter_string(config))
        .unwrap_or_else(|_| EnvFilter::new("deepseekcode=info"));

    let console = resolve_console_enabled(config);
    let file = config.file;
    let audit = config.audit.enabled;
    let pretty = matches!(config.format, LogFormat::Pretty);

    if file {
        let (app_w, g1) = open_log(&run_dir, "app.log")?;
        let (sess_w, g2) = open_log(&run_dir, "session.log")?;
        push_guard(g1);
        push_guard(g2);
        let session_filter =
            tracing_subscriber::filter::filter_fn(|m| crate::targets::is_session_target(m.target()));

        if audit {
            let (audit_w, g3) = open_log(&run_dir, "audit.log")?;
            push_guard(g3);
            let audit_filter =
                tracing_subscriber::filter::filter_fn(|m| crate::targets::is_audit_target(m.target()));
            if console && pretty {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                    .with(fmt::layer().json().with_writer(audit_w).with_filter(audit_filter))
                    .with(fmt::layer().pretty().with_writer(io::stdout))
                    .init();
            } else if console {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                    .with(fmt::layer().json().with_writer(audit_w).with_filter(audit_filter))
                    .with(fmt::layer().json().with_writer(io::stdout))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(app_w))
                    .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                    .with(fmt::layer().json().with_writer(audit_w).with_filter(audit_filter))
                    .init();
            }
        } else if console && pretty {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                .with(fmt::layer().pretty().with_writer(io::stdout))
                .init();
        } else if console {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                .with(fmt::layer().json().with_writer(io::stdout))
                .init();
        } else {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(app_w))
                .with(fmt::layer().json().with_writer(sess_w).with_filter(session_filter))
                .init();
        }
    } else if console && pretty {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty().with_writer(io::stdout))
            .init();
    } else if console {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_writer(io::stdout))
            .init();
    } else {
        tracing_subscriber::registry().with(filter).init();
    }

    install_panic_hook(&run_dir);
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
    run_dir: &PathBuf,
    name: &str,
) -> Result<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join(name))
        .map_err(DeepSeekCodeError::Io)?;
    Ok(tracing_appender::non_blocking(file))
}

fn push_guard(guard: tracing_appender::non_blocking::WorkerGuard) {
    if let Ok(mut guards) = GUARDS.lock() {
        guards.push(guard);
    }
}

fn install_panic_hook(run_dir: &PathBuf) {
    let dir = run_dir.clone();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("panic: {info}");
        tracing::error!(target: crate::targets::APP, "{msg}");
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
