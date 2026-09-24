use deepseekcode_schema::agent::AgentKind;
use deepseekcode_schema::api::{
    AddProjectDirectoryRequest, CreatePtyRequest, CreateWorktreeRequest, ForkRequest,
    McpConnectResponse, MoveSessionRequest, OkResponse, PluginInfo, ProjectDirectoryEntry,
    SessionExportResponse, SkillInfo, TimelineEntry, UpdateModelRequest, WorktreeEntry,
};
use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::message::Message;
use deepseekcode_schema::permission::{PermissionRequest, PermissionStatus};
use deepseekcode_schema::plan::{PlanCompletedPayload, PlanMeta, SessionPlanSummary};
use deepseekcode_schema::policy::{
    PolicyAction, PolicyGrant, PolicyReply, PolicyReplyKind, PolicyRequest, PolicyRequestStatus,
    PolicyRule,
};
use deepseekcode_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest, Session};
use schemars::schema_for;
use std::io::Write;

fn main() {
    let schemas: Vec<(&str, schemars::schema::RootSchema)> = vec![
        ("event_kind", schema_for!(EventKind)),
        ("event_envelope", schema_for!(EventEnvelope)),
        ("session", schema_for!(Session)),
        ("message", schema_for!(Message)),
        ("create_session_request", schema_for!(CreateSessionRequest)),
        ("prompt_request", schema_for!(PromptRequest)),
        ("prompt_delivery", schema_for!(PromptDelivery)),
        ("agent_kind", schema_for!(AgentKind)),
        ("permission_request", schema_for!(PermissionRequest)),
        ("permission_status", schema_for!(PermissionStatus)),
        ("policy_action", schema_for!(PolicyAction)),
        ("policy_rule", schema_for!(PolicyRule)),
        ("policy_request", schema_for!(PolicyRequest)),
        ("policy_reply", schema_for!(PolicyReply)),
        ("policy_reply_kind", schema_for!(PolicyReplyKind)),
        ("policy_request_status", schema_for!(PolicyRequestStatus)),
        ("policy_grant", schema_for!(PolicyGrant)),
        ("plan_meta", schema_for!(PlanMeta)),
        ("plan_completed_payload", schema_for!(PlanCompletedPayload)),
        ("session_plan_summary", schema_for!(SessionPlanSummary)),
        ("ok_response", schema_for!(OkResponse)),
        ("move_session_request", schema_for!(MoveSessionRequest)),
        ("update_model_request", schema_for!(UpdateModelRequest)),
        ("skill_info", schema_for!(SkillInfo)),
        ("plugin_info", schema_for!(PluginInfo)),
        (
            "session_export_response",
            schema_for!(SessionExportResponse),
        ),
        ("timeline_entry", schema_for!(TimelineEntry)),
        ("worktree_entry", schema_for!(WorktreeEntry)),
        (
            "project_directory_entry",
            schema_for!(ProjectDirectoryEntry),
        ),
        (
            "add_project_directory_request",
            schema_for!(AddProjectDirectoryRequest),
        ),
        (
            "create_worktree_request",
            schema_for!(CreateWorktreeRequest),
        ),
        ("mcp_connect_response", schema_for!(McpConnectResponse)),
        ("fork_request", schema_for!(ForkRequest)),
        ("create_pty_request", schema_for!(CreatePtyRequest)),
    ];

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/schema".to_string());
    std::fs::create_dir_all(&out_dir).expect("create schema dir");

    for (name, schema) in schemas {
        let path = format!("{out_dir}/{name}.json");
        let json = serde_json::to_string_pretty(&schema).expect("serialize schema");
        let mut file = std::fs::File::create(&path).expect("create schema file");
        file.write_all(json.as_bytes()).expect("write schema");
        eprintln!("wrote {path}");
    }
}
