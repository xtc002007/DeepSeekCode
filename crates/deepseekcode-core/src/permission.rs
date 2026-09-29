//! Backward-compatible re-export; implementation lives in `policy/`.
//! Product logging for permission decisions lives in `policy/service.rs`
//! (AUDIT target) and `policy/evaluate.rs`; this shim carries no executable
//! path itself so it emits no separate `tracing!` sites by design.
pub use crate::policy::{PermissionService, PolicyOutcome, PolicyService};
