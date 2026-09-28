//! Smoke benchmark: SSE first-event latency (docs/architecture.md: < 50ms locally).

use std::time::{Duration, Instant};
use tempfile::TempDir;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# bench").expect("agents");
    let db_path = dir.path().join("bench.db");
    std::env::set_var("DEEPSEEKCODE_DB_PATH", &db_path);

    let elapsed = rt.block_on(async {
        let embedded = deepseekcode_sdk::EmbeddedEngine::start(dir.path().to_string_lossy())
            .await
            .expect("embedded engine");
        let client = embedded.client();

        let session = client
            .create_session(deepseekcode_schema::session::CreateSessionRequest {
                id: None,
                directory: dir.path().to_string_lossy().to_string(),
                agent: None,
                model_id: None,
                provider_id: None,
            })
            .await
            .expect("session");

        let _ = session;
        let mut stream = client.event_stream(0, None).await.expect("event stream");
        let start = Instant::now();
        let first = tokio::time::timeout(Duration::from_secs(10), async {
            use futures::StreamExt;
            stream.next().await
        })
        .await
        .expect("timeout waiting for first SSE event");

        first.expect("stream ended");
        start.elapsed()
    });

    println!("sse_first_event_ms={}", elapsed.as_millis());

    let threshold = if std::env::var("CI").is_ok() {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(50)
    };
    assert!(
        elapsed < threshold,
        "sse latency bench: {:?}, threshold {:?}",
        elapsed,
        threshold
    );
}
