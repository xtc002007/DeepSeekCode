//! Session domain surface. Per-turn product logging lives in
//! `service.rs` / `execution.rs` / `runner/*` (SESSION target with run_id +
//! session_id); this module only re-exports so it emits no separate
//! `tracing!` sites.
pub mod execution;
pub mod projector;
pub mod revert;
pub mod runner;
pub mod service;

pub use execution::SessionExecution;
pub use projector::EventProjector;
pub use revert::{RevertManager, StagedRevert};
pub use runner::SessionRunner;
pub use service::SessionService;
