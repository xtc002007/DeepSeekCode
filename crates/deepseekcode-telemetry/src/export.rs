use crate::context::RunContext;
use crate::redact::redact_log_content;
use deepseekcode_types::{DeepSeekCodeError, Result, DIAGNOSTICS_ARCHIVE_PREFIX};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Delete all previous `run-*` directories under `logs_root`.
pub fn clear_previous_runs(logs_root: &Path) -> Result<()> {
    let entries = match fs::read_dir(logs_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(DeepSeekCodeError::Io(e)),
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("run-"))
        {
            let _ = fs::remove_dir_all(path);
        }
    }
    Ok(())
}

/// Remove oldest run directories beyond `max_run_dirs`.
pub fn prune_old_runs(logs_root: &Path, max_run_dirs: usize) -> Result<()> {
    let mut runs: Vec<PathBuf> = fs::read_dir(logs_root)
        .map_err(DeepSeekCodeError::Io)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("run-"))
                .unwrap_or(false)
        })
        .collect();
    runs.sort();
    if runs.len() <= max_run_dirs {
        return Ok(());
    }
    let remove_count = runs.len() - max_run_dirs;
    for path in runs.into_iter().take(remove_count) {
        let _ = fs::remove_dir_all(path);
    }
    Ok(())
}

/// Export a diagnostic zip bundle for the given run (or current run if None).
/// When `redact` is true (default), log file contents are scrubbed for secrets.
pub fn export_diagnostic_bundle(run_id: Option<&str>, output: &Path) -> Result<PathBuf> {
    export_diagnostic_bundle_with_options(run_id, output, true)
}

pub fn export_diagnostic_bundle_with_options(
    run_id: Option<&str>,
    output: &Path,
    redact: bool,
) -> Result<PathBuf> {
    let ctx = RunContext::global()
        .ok_or_else(|| DeepSeekCodeError::Other("telemetry not initialized".into()))?;

    let run_dir = if let Some(id) = run_id {
        ctx.logs_root.join(format!("run-{id}"))
    } else {
        ctx.run_dir.clone()
    };

    if !run_dir.exists() {
        return Err(DeepSeekCodeError::NotFound(format!(
            "run directory not found: {}",
            run_dir.display()
        )));
    }

    let out_path = if output.extension().is_some_and(|e| e == "zip") {
        output.to_path_buf()
    } else {
        output.join(format!(
            "{DIAGNOSTICS_ARCHIVE_PREFIX}-{}.zip",
            run_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        ))
    };

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(DeepSeekCodeError::Io)?;
    }

    let file = File::create(&out_path).map_err(DeepSeekCodeError::Io)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in WalkDir::new(&run_dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let name = path
            .strip_prefix(&run_dir)
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        zip.start_file(name, options)
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;
        let mut buf = Vec::new();
        File::open(path)
            .and_then(|mut f| f.read_to_end(&mut buf))
            .map_err(DeepSeekCodeError::Io)?;
        if redact && should_redact_diagnostic_file(path) {
            let text = String::from_utf8_lossy(&buf);
            buf = redact_log_content(&text).into_bytes();
        }
        zip.write_all(&buf).map_err(DeepSeekCodeError::Io)?;
    }

    zip.finish()
        .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;

    Ok(out_path)
}

fn should_redact_diagnostic_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "log" | "json" | "txt"))
}

/// Read the last N lines from app.log for CI / failure dumps.
/// Streaming reverse scan (P1-13: previously `read_to_string` loaded the
/// whole file, OOM on large logs).
pub fn tail_app_log(run_dir: &Path, lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = run_dir.join("app.log");
    let Ok(mut file) = fs::File::open(&path) else {
        return String::new();
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return String::new();
    };
    // Scan backwards in 8KiB windows, collecting newlines until `lines`.
    const WINDOW: u64 = 8192;
    let mut buf = Vec::new();
    let mut pos = size;
    let mut newlines = 0usize;
    while pos > 0 && newlines <= lines {
        let take = WINDOW.min(pos);
        pos -= take;
        if file.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut chunk = vec![0u8; take as usize];
        if file.read_exact(&mut chunk).is_err() {
            break;
        }
        newlines += chunk.iter().filter(|b| **b == b'\n').count();
        buf.splice(0..0, chunk);
        if buf.len() > 1024 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    text.lines()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
