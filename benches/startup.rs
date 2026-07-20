//! Smoke benchmark: embedded engine cold start target < 2s (see docs/architecture.md).

use std::time::Instant;
use tempfile::TempDir;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# bench").expect("agents");

    let start = Instant::now();
    rt.block_on(async {
        let _ = deepseekcode_sdk::EmbeddedEngine::start(dir.path().to_string_lossy()).await;
    });
    let elapsed = start.elapsed();
    println!("embedded_startup_ms={}", elapsed.as_millis());
    assert!(
        elapsed.as_secs()
            < if std::env::var("CI").is_ok() { 3 } else { 2 },
        "startup smoke threshold: {:?}",
        elapsed
    );
}
