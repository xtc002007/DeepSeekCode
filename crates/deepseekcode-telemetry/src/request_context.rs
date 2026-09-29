//! Task-local HTTP request id shared across crates without cycles.
//!
//! `deepseekcode-embed` sets it per request; `deepseekcode-core` background
//! tasks (e.g. session drain spawns) capture + re-scope it so their logs join
//! 1:1 with the originating HTTP logs. Outside HTTP (CLI direct, tests) the
//! id is `"-"`. `core` must not depend on `embed`, hence this lives here.

tokio::task_local! {
    static CURRENT_REQUEST_ID: String
}

/// Non-panicking read. Returns `"-"` when no request scope is set.
pub fn current_request_id() -> String {
    CURRENT_REQUEST_ID
        .try_with(|id| id.clone())
        .unwrap_or_else(|_| "-".to_string())
}

/// Run `fut` with `id` visible to `current_request_id()`.
pub async fn scope_request_id<F, T>(id: String, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_REQUEST_ID.scope(id, fut).await
}
