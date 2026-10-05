//! Canonical session-status text → localized display text.
//!
//! `WorkspaceView::session_status` stores an English canonical token (built by
//! `format!` at the event-handling site, never translated, never user input).
//! Comparisons (`== "Ready"`, `== "Loading session…"`) keep working on that
//! token. Only the single render site goes through here.
//!
//! Matching our own canonical format is safe. What must never come back is
//! scraping numbers or structure out of *translated* text — that is what the
//! old `parse_worked_secs` / `humanize_worked_label` pair did with
//! `" for "`, and it broke in every non-English locale.

/// Render a canonical status token for display in the active locale.
///
/// Unknown tokens pass through unchanged so a future status never renders
/// blank while its catalog entry is missing.
pub fn localize_session_status(status: &str) -> String {
    use deepseekcode_i18n::tr;
    match status {
        "Ready" => tr!("status.ready"),
        "Error" => tr!("status.error"),
        "Interrupted" => tr!("status.interrupted"),
        "Session" => tr!("status.session"),
        "Loading session…" => tr!("status.loading_session"),
        "Running…" => tr!("status.running"),
        "Thinking…" => tr!("status.thinking"),
        "Generating response…" => tr!("status.generating"),
        "Continuing analysis…" => tr!("status.continuing"),
        "Running tools…" => tr!("status.running_tools"),
        "Updating context…" => tr!("status.updating_context"),
        "Prompt queued…" => tr!("status.prompt_queued"),
        "Waiting for policy approval…" => tr!("status.waiting_policy"),
        "Compacting…" => tr!("status.compacting"),
        "Sending prompt…" => tr!("workspace.sending_prompt"),
        "Stopping…" => tr!("workspace.stopping"),
        _ => {
            if let Some(tool) = status.strip_prefix("Waiting for policy: ") {
                return tr!("status.waiting_policy_named", "tool" => tool);
            }
            if let Some(name) = status.strip_prefix("Running tool: ") {
                return tr!("status.running_tool_named", "name" => name);
            }
            if let Some(rest) = status.strip_prefix("Calling model… (") {
                if let Some(rendered) = localize_calling_model(rest) {
                    return rendered;
                }
            }
            status.to_string()
        }
    }
}

/// `turn {t}/{m}, {c} msgs)` / `turn {t}, {c} msgs)` / `{c} msgs)` — the three
/// canonical shapes built at the `ProviderTurnStarted` site.
fn localize_calling_model(rest: &str) -> Option<String> {
    use deepseekcode_i18n::tr;
    let rest = rest.strip_suffix(')')?;
    if let Some(turn_part) = rest.strip_prefix("turn ") {
        // `turn {t}/{m}, {c} msgs` or `turn {t}, {c} msgs`.
        let (turn_str, count_str) = turn_part.split_once(", ")?;
        let count: u64 = count_str.strip_suffix(" msgs")?.trim().parse().ok()?;
        if let Some((turn_str, max_str)) = turn_str.split_once('/') {
            let turn: u64 = turn_str.trim().parse().ok()?;
            let max_turns: u64 = max_str.trim().parse().ok()?;
            return Some(tr!(
                "status.calling_model_full",
                "turn" => turn,
                "max_turns" => max_turns,
                "message_count" => count
            ));
        }
        let turn: u64 = turn_str.trim().parse().ok()?;
        return Some(tr!(
            "status.calling_model_turn",
            "turn" => turn,
            "message_count" => count
        ));
    }
    let count: u64 = rest.strip_suffix(" msgs")?.trim().parse().ok()?;
    Some(tr!("status.calling_model", "message_count" => count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_tokens_localize() {
        assert_eq!(
            localize_session_status("Ready"),
            deepseekcode_i18n::t("status.ready")
        );
        assert_eq!(
            localize_session_status("Waiting for policy: bash"),
            deepseekcode_i18n::tr!("status.waiting_policy_named", "tool" => "bash")
        );
        assert_eq!(
            localize_session_status("Calling model… (turn 2/8, 41 msgs)"),
            deepseekcode_i18n::tr!(
                "status.calling_model_full",
                "turn" => 2u64,
                "max_turns" => 8u64,
                "message_count" => 41u64
            )
        );
        assert_eq!(
            localize_session_status("Calling model… (7 msgs)"),
            deepseekcode_i18n::tr!("status.calling_model", "message_count" => 7u64)
        );
        // Unknown future tokens pass through instead of rendering blank.
        assert_eq!(localize_session_status("Frobnicate…"), "Frobnicate…");
    }
}
