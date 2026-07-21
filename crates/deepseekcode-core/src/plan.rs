//! Plan workflow helpers.

use deepseekcode_schema::event::{EventEnvelope, EventKind};
use deepseekcode_schema::plan::SessionPlanSummary;
use std::collections::HashSet;

/// Collect completed plans for a session from durable events.
pub fn session_plans_from_events(
    events: &[EventEnvelope],
    dismissed: &HashSet<String>,
) -> Vec<SessionPlanSummary> {
    let mut plans = Vec::new();
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
        plans.push(SessionPlanSummary {
            plan_id: plan_id.clone(),
            plan_path,
            title,
            summary,
            completed_at: event.created_at.to_rfc3339(),
            implement_prompt_dismissed: dismissed.contains(&plan_id),
        });
    }
    plans
}

/// Latest non-dismissed plan id, if any.
pub fn latest_actionable_plan_id(plans: &[SessionPlanSummary]) -> Option<String> {
    plans
        .iter()
        .filter(|p| !p.implement_prompt_dismissed)
        .last()
        .map(|p| p.plan_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use deepseekcode_schema::event::EventEnvelope;

    fn plan_event(plan_id: &str, title: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 1,
            kind: EventKind::PlanCompleted,
            session_id: None,
            payload: serde_json::json!({
                "plan_id": plan_id,
                "plan_path": format!(".deepseekcode/plan/{plan_id}/PLAN.md"),
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
