//! Smoke benchmark: process RSS after embedded engine start (docs/architecture.md).

use std::time::Instant;
use tempfile::TempDir;

#[cfg(target_os = "linux")]
fn current_rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(kb) = line.strip_prefix("VmRSS:").map(str::trim) {
            let kb = kb.trim_end_matches(" kB").parse::<u64>().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn current_rss_mb() -> Option<u64> {
    None
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# bench").expect("agents");
    let db_path = dir.path().join("bench.db");
    std::env::set_var("DEEPSEEKCODE_DB_PATH", &db_path);

    let before = current_rss_mb();
    let start = Instant::now();
    rt.block_on(async {
        let _ = deepseekcode_sdk::EmbeddedEngine::start(dir.path().to_string_lossy())
            .await
            .expect("embedded engine");
    });
    let after = current_rss_mb();
    let elapsed = start.elapsed();

    if let (Some(before), Some(after)) = (before, after) {
        let delta = after.saturating_sub(before);
        println!(
            "memory_rss_mb={after} delta_mb={delta} startup_ms={}",
            elapsed.as_millis()
        );
        let threshold_mb = if std::env::var("CI").is_ok() { 400 } else { 150 };
        assert!(
            after < threshold_mb,
            "idle RSS {after}MB exceeds threshold {threshold_mb}MB"
        );
    } else {
        println!(
            "memory_rss_mb=unavailable startup_ms={}",
            elapsed.as_millis()
        );
        eprintln!("memory bench: RSS probe unavailable on this platform");
    }
}
