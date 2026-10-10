//! Core domain engine. This crate root only declares modules + re-exports;
//! product `tracing!` sites live in submodules (e.g. `session/service.rs`,
//! `git/project.rs`, `mcp/pool.rs`, `policy/service.rs`,
//! `system_context/registry.rs`), never at this `mod` surface.
//! `git/mod.rs`, `mcp/mod.rs`, `policy/mod.rs`, `session/mod.rs`,
//! `system_context/mod.rs` document their submodule log ownership inline.
pub mod activity;
pub mod agent;
pub mod cache_status;
pub mod compaction;
pub mod engine;
pub mod git;
pub mod location;
pub mod mcp;
pub mod memory;
pub mod permission;
pub mod plan;
pub mod policy;
pub mod pricing;
pub mod process;
pub mod project_context;
pub mod provider;
pub mod session;
pub mod shell_env;
pub mod skills;
pub mod snapshot;
pub mod system_context;
pub mod todo;
pub mod token;
pub mod tools;

pub use activity::{
    ActivityApplyOutcome, ActivityProjectionState, ActivityProjector, ChildSessionProjection,
    SESSION_EVENTS_CAP,
};
pub use engine::DeepSeekCodeEngine;
pub use plan::{latest_actionable_plan_id, session_plans_from_events};
pub use snapshot::FileSnapshotStore;
pub use todo::TodoStore;
