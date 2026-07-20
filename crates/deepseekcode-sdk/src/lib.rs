pub mod client;
pub mod embedded;

pub use client::{DeepSeekCodeClient, EventStreamItem, FsEntry, FsTreePage, PluginInfo, SkillInfo};
pub use embedded::EmbeddedEngine;
pub use deepseekcode_core::activity::{
    event_requires_full_rebuild, patch_assistant_message_completed, patch_assistant_message_delta,
    patch_running_tool_leaf_preview, patch_tool_completed, patch_tool_started, ActivityApplyOutcome,
    ActivityProjectionState, ActivityProjector, ChildSessionProjection, SESSION_EVENTS_CAP,
};
pub use deepseekcode_core::session_plans_from_events;
