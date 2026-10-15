pub mod incremental;
pub mod projector;

use code_plus_plus_schema::event::EventKind;
use code_plus_plus_storage::EventStore;
use code_plus_plus_types::{Result, SessionId};
use serde_json::json;

pub use incremental::{
    event_requires_full_rebuild, patch_assistant_message_completed, patch_assistant_message_delta,
    patch_running_tool_leaf_preview, patch_tool_completed, patch_tool_started,
    ActivityApplyOutcome, ActivityProjectionState, SESSION_EVENTS_CAP,
};
pub use projector::{ActivityProjector, ChildSessionProjection};

pub async fn emit_activity_updated(
    events: &EventStore,
    session_id: SessionId,
    since_seq: i64,
) -> Result<()> {
    tracing::debug!(
        target: code_plus_plus_telemetry::SESSION,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        session_id = %session_id,
        since_seq = since_seq,
        "emit_activity_updated started"
    );
    let r = events
        .append(
            EventKind::ActivityUpdated,
            Some(session_id),
            json!({ "since_seq": since_seq }),
        )
        .await;
    if let Err(e) = &r {
        tracing::error!(
            target: code_plus_plus_telemetry::SESSION,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            session_id = %session_id,
            error = %e,
            "emit_activity_updated failed"
        );
    }
    r?;
    Ok(())
}
