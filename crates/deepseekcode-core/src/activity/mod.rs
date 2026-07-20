pub mod incremental;
pub mod projector;

use deepseekcode_schema::event::EventKind;
use deepseekcode_storage::EventStore;
use deepseekcode_types::{Result, SessionId};
use serde_json::json;

pub use incremental::{
    event_requires_full_rebuild, patch_assistant_message_completed, patch_assistant_message_delta,
    patch_running_tool_leaf_preview, patch_tool_completed, patch_tool_started, ActivityApplyOutcome,
    ActivityProjectionState, SESSION_EVENTS_CAP,
};
pub use projector::{ActivityProjector, ChildSessionProjection};

pub async fn emit_activity_updated(
    events: &EventStore,
    session_id: SessionId,
    since_seq: i64,
) -> Result<()> {
    events
        .append(
            EventKind::ActivityUpdated,
            Some(session_id),
            json!({ "since_seq": since_seq }),
        )
        .await?;
    Ok(())
}
