//! Smoke benchmark: directory listing with ignore crate (docs/architecture.md: 10k nodes < 200ms).

use std::time::{Duration, Instant};
use tempfile::TempDir;

fn main() {
    let tmp = TempDir::new().expect("tempdir");
    for i in 0..10_000 {
        std::fs::write(tmp.path().join(format!("node_{i}.txt")), "x").expect("write node");
    }

    let start = Instant::now();
    let walker = ignore::WalkBuilder::new(tmp.path())
        .hidden(true)
        .git_ignore(false)
        .build();
    let count = walker.flatten().count();
    let elapsed = start.elapsed();

    println!("file_tree_entries={count} elapsed_ms={}", elapsed.as_millis());

    let threshold = if std::env::var("CI").is_ok() {
        Duration::from_millis(2_000)
    } else {
        Duration::from_millis(200)
    };
    assert!(
        elapsed < threshold,
        "file_tree bench: {count} entries in {:?}, threshold {:?}",
        elapsed,
        threshold
    );
}
