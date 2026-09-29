//! System context surface. Per-load product logging lives in
//! `registry.rs` (SESSION target) and `source.rs` (APP target for remote
//! fetch); this module only re-exports so it emits no separate `tracing!`
//! sites.
pub mod epoch;
pub mod registry;
pub mod source;

pub use epoch::{ContextEpoch, ContextEpochStore};
pub use registry::SystemContextRegistry;
pub use source::{ContextKey, ContextSource, LoadResult, ReconcileResult, Snapshot};
