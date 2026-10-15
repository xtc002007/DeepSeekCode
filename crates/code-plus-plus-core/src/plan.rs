//! Plan workflow helpers.

use code_plus_plus_schema::event::{EventEnvelope, EventKind};
use code_plus_plus_schema::plan::SessionPlanSummary;
use std::collections::HashSet;

/// Collect completed plans for a session from durable events.
/// Deduplicated by `plan_id` (latest event wins) and sorted by
/// `completed_at` so reordered replays select the same actionable plan
/// (P0-8: previously duplicates accumulated and `rfind` depended on log order).
pub fn session_plans_from_events(
    events: &[EventEnvelope],
    dismissed: &HashSet<String>,
) -> Vec<SessionPlanSummary> {
    tracing::debug!(
        target: code_plus_plus_telemetry::APP,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        events = events.len(),
        dismissed = dismissed.len(),
        "session_plans_from_events started"
    );
    use std::collections::HashMap;
    let mut latest: HashMap<String, SessionPlanSummary> = HashMap::new();
    for event in events {
        if event.kind != EventKind::PlanCompleted {
            continue;
        }
        let plan_id = event.payload["plan_id"].as_str().unwrap_or("").to_string();
        if plan_id.is_empty() {
            continue;
        }
        let plan_path = event.payload["plan_path"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let title = event.payload["title"]
            .as_str()
            .unwrap_or("Plan")
            .to_string();
        let summary = event.payload["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from);
        latest.insert(
            plan_id.clone(),
            SessionPlanSummary {
                plan_id: plan_id.clone(),
                plan_path,
                title,
                summary,
                completed_at: event.created_at.to_rfc3339(),
                implement_prompt_dismissed: dismissed.contains(&plan_id),
            },
        );
    }
    let mut plans: Vec<_> = latest.into_values().collect();
    plans.sort_by(|a, b| {
        a.completed_at
            .cmp(&b.completed_at)
            .then(a.plan_id.cmp(&b.plan_id))
    });
    tracing::info!(
        target: code_plus_plus_telemetry::APP,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        plans = plans.len(),
        "session_plans_from_events completed"
    );
    plans
}

/// Latest non-dismissed plan id, if any (by `completed_at` order).
pub fn latest_actionable_plan_id(plans: &[SessionPlanSummary]) -> Option<String> {
    tracing::debug!(
        target: code_plus_plus_telemetry::APP,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        plans = plans.len(),
        "latest_actionable_plan_id queried"
    );
    plans
        .iter()
        .filter(|p| !p.implement_prompt_dismissed)
        .max_by(|a, b| {
            a.completed_at
                .cmp(&b.completed_at)
                .then(a.plan_id.cmp(&b.plan_id))
        })
        .map(|p| p.plan_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use code_plus_plus_schema::event::EventEnvelope;

    fn plan_event(plan_id: &str, title: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 1,
            kind: EventKind::PlanCompleted,
            session_id: None,
            payload: serde_json::json!({
                "plan_id": plan_id,
                "plan_path": format!(".code-plus-plus/plan/{plan_id}/PLAN.md"),
                "title": title,
            }),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn collects_plans_and_dismiss_state() {
        let events = vec![plan_event("p1", "First"), plan_event("p2", "Second")];
        let mut dismissed = HashSet::new();
        dismissed.insert("p1".into());
        let plans = session_plans_from_events(&events, &dismissed);
        assert_eq!(plans.len(), 2);
        assert!(plans[0].implement_prompt_dismissed);
        assert!(!plans[1].implement_prompt_dismissed);
        assert_eq!(latest_actionable_plan_id(&plans).as_deref(), Some("p2"));
    }
}
