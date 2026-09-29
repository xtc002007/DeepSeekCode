//! High-level SDK + embedded engine. This crate root only re-exports;
//! product `tracing!` sites live in `client.rs` (APP target, `run_id` +
//! `operation` + `latency_ms`, never full prompt/key) and `embedded.rs`
//! (engine lifecycle).
pub mod client;
pub mod embedded;

pub use client::{DeepSeekCodeClient, EventStreamItem, FsEntry, FsTreePage, PluginInfo, SkillInfo};
pub use deepseekcode_core::activity::{
    event_requires_full_rebuild, patch_assistant_message_completed, patch_assistant_message_delta,
    patch_running_tool_leaf_preview, patch_tool_completed, patch_tool_started,
    ActivityApplyOutcome, ActivityProjectionState, ActivityProjector, ChildSessionProjection,
    SESSION_EVENTS_CAP,
};
pub use deepseekcode_core::session_plans_from_events;
pub use embedded::{install_dedicated_engine_runtime, EmbeddedEngine};
