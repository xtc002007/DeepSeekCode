//! One-line leaf labels for Cursor-style activity rows.

use deepseekcode_schema::activity::{
    ActivityKind, ActivityLeaf, ActivityMeta, ActivityStatus, ActivitySummary, ActivityTargetKind,
};

pub fn format_summary_line(summary: &ActivitySummary) -> String {
    let mut parts = Vec::new();
    if summary.files_edited > 0 {
        parts.push(format!("Edited {} files", summary.files_edited));
    }
    if summary.files_explored > 0 {
        parts.push(format!("explored {} files", summary.files_explored));
    }
    if summary.searches > 0 {
        parts.push(format!("{} searches", summary.searches));
    }
    if summary.commands > 0 {
        parts.push(format!("ran {} commands", summary.commands));
    }
    let mut line = parts.join(", ");
    if summary.lines_added > 0 || summary.lines_removed > 0 {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&format!(
            "+{} -{}",
            summary.lines_added, summary.lines_removed
        ));
    }
    if line.is_empty() {
        "Activity".into()
    } else {
        line
    }
}

pub fn format_message_preview(verb: &str, text: &str) -> String {
    let preview: String = text.lines().take(2).collect::<Vec<_>>().join(" ");
    let truncated = if preview.chars().count() > 120 {
        format!("{}…", preview.chars().take(120).collect::<String>())
    } else {
        preview
    };
    if truncated.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} · {truncated}")
    }
}

pub fn running_prefix(status: ActivityStatus, _kind: ActivityKind) -> &'static str {
    if status == ActivityStatus::Running {
        "◐ "
    } else {
        ""
    }
}

pub fn format_leaf_line(leaf: &ActivityLeaf) -> String {
    let prefix = running_prefix(leaf.status, leaf.kind);

    if leaf.kind == ActivityKind::UserMessage {
        if let Some(text) = &leaf.text {
            return format!("{prefix}{}", format_message_preview("You", text));
        }
    }
    if leaf.kind == ActivityKind::AssistantMessage {
        if let Some(text) = &leaf.text {
            return format!("{prefix}{}", format_message_preview("Assistant", text));
        }
    }
    if leaf.kind == ActivityKind::Permission {
        return format!("{prefix}{}", leaf.verb);
    }
    if leaf.status == ActivityStatus::Running && leaf.kind == ActivityKind::Bash {
        if let ActivityTargetKind::Command { command } = &leaf.target {
            let cmd = if command.chars().count() > 72 {
                format!("{}…", command.chars().take(72).collect::<String>())
            } else {
                command.clone()
            };
            let mut line = format!("{prefix}Running {cmd}");
            if let Some(text) = &leaf.text {
                let preview = bash_stream_preview(text);
                if !preview.is_empty() {
                    line.push_str(" · ");
                    line.push_str(&preview);
                }
            }
            return line;
        }
    }

    let object = format_target(&leaf.target);
    let meta = match (&leaf.kind, &leaf.target) {
        (
            ActivityKind::Read,
            ActivityTargetKind::File {
                line_start,
                line_end,
                ..
            },
        ) => format_read_meta(*line_start, *line_end),
        _ => format_meta(&leaf.kind, &leaf.meta),
    };
    let body = if object.is_empty() {
        if meta.is_empty() {
            leaf.verb.clone()
        } else {
            format!("{} {}", leaf.verb, meta)
        }
    } else if meta.is_empty() {
        format!("{} {}", leaf.verb, object)
    } else {
        format!("{} {} {}", leaf.verb, object, meta)
    };
    format!("{prefix}{body}")
}

fn format_target(target: &ActivityTargetKind) -> String {
    match target {
        ActivityTargetKind::File { path, .. } => shorten_path(path),
        ActivityTargetKind::Command { command } => {
            if command.chars().count() > 72 {
                format!("{}…", command.chars().take(72).collect::<String>())
            } else {
                command.clone()
            }
        }
        ActivityTargetKind::Pattern { pattern, .. } => pattern.clone(),
        ActivityTargetKind::Url { url } => url.clone(),
        ActivityTargetKind::Session { label, session_id } => {
            label.clone().unwrap_or_else(|| session_id.clone())
        }
        ActivityTargetKind::TodoList => "todos".into(),
        ActivityTargetKind::Skill { name } => name.clone(),
        ActivityTargetKind::Generic { label } => label.clone(),
        ActivityTargetKind::None => String::new(),
    }
}

fn format_meta(kind: &ActivityKind, meta: &ActivityMeta) -> String {
    match kind {
        ActivityKind::Write | ActivityKind::Edit | ActivityKind::ApplyPatch => diff_meta(meta),
        ActivityKind::Grep | ActivityKind::WebSearch => meta
            .match_count
            .map(|c| format!("{c} matches"))
            .unwrap_or_default(),
        ActivityKind::Glob | ActivityKind::List => meta
            .entry_count
            .or(meta.file_count)
            .map(|c| format!("{c} items"))
            .unwrap_or_default(),
        ActivityKind::Bash => {
            if meta.exit_code.is_some() {
                meta.exit_code
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_default()
            } else if meta.duration_ms.is_some() {
                meta.duration_ms
                    .map(|ms| format!("{ms}ms"))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        ActivityKind::TodoWrite => {
            let pending = meta
                .todo_pending
                .map(|c| format!("{c} pending"))
                .unwrap_or_default();
            let active = meta
                .todo_in_progress
                .map(|c| format!("{c} active"))
                .unwrap_or_default();
            match (pending.is_empty(), active.is_empty()) {
                (false, false) => format!("{pending}, {active}"),
                (false, true) => pending,
                (true, false) => active,
                (true, true) => String::new(),
            }
        }
        ActivityKind::Reasoning => meta
            .duration_ms
            .map(|ms| {
                let secs = ms / 1000;
                if secs == 0 {
                    "<1s".into()
                } else {
                    format!("{secs}s")
                }
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub fn format_read_meta(line_start: Option<u32>, line_end: Option<u32>) -> String {
    match (line_start, line_end) {
        (Some(s), Some(e)) if s == e => format!("L{s}"),
        (Some(s), Some(e)) => format!("L{s}-{e}"),
        (Some(s), None) => format!("L{s}"),
        _ => String::new(),
    }
}

fn diff_meta(meta: &ActivityMeta) -> String {
    meta.diff_stats
        .as_ref()
        .map(|d| format!("+{} -{}", d.added, d.removed))
        .unwrap_or_default()
}

fn bash_stream_preview(text: &str) -> String {
    let last = text
        .lines()
        .map(|line| {
            line.strip_prefix("[stdout] ")
                .or_else(|| line.strip_prefix("[stderr] "))
                .unwrap_or(line)
        })
        .filter(|line| !line.is_empty())
        .last()
        .unwrap_or("");
    if last.chars().count() > 48 {
        format!("{}…", last.chars().take(48).collect::<String>())
    } else {
        last.to_string()
    }
}

fn shorten_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        p
    } else {
        format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseekcode_schema::activity::{ActivityStatus, DiffStats};

    #[test]
    fn formats_edit_leaf() {
        let leaf = ActivityLeaf {
            id: "1".into(),
            kind: ActivityKind::Edit,
            verb: "Edited".into(),
            target: ActivityTargetKind::File {
                path: "src/main.rs".into(),
                line_start: None,
                line_end: None,
            },
            meta: ActivityMeta {
                diff_stats: Some(DiffStats {
                    added: 21,
                    removed: 1,
                }),
                ..Default::default()
            },
            status: ActivityStatus::Completed,
            detail_ref: None,
            text: None,
        };
        assert!(format_leaf_line(&leaf).contains("Edited"));
        assert!(format_leaf_line(&leaf).contains("+21 -1"));
    }

    #[test]
    fn formats_user_message_with_body_preview() {
        let leaf = ActivityLeaf {
            id: "u1".into(),
            kind: ActivityKind::UserMessage,
            verb: "You".into(),
            target: ActivityTargetKind::None,
            meta: ActivityMeta::default(),
            status: ActivityStatus::Completed,
            detail_ref: None,
            text: Some("请分析这个项目的架构设计".into()),
        };
        let line = format_leaf_line(&leaf);
        assert!(line.starts_with("You · "));
        assert!(line.contains("架构"));
    }

    #[test]
    fn formats_assistant_message_with_preview() {
        let leaf = ActivityLeaf {
            id: "a1".into(),
            kind: ActivityKind::AssistantMessage,
            verb: "Assistant".into(),
            target: ActivityTargetKind::None,
            meta: ActivityMeta::default(),
            status: ActivityStatus::Completed,
            detail_ref: None,
            text: Some("Here is the analysis of your project.".into()),
        };
        let line = format_leaf_line(&leaf);
        assert!(line.starts_with("Assistant · "));
        assert!(line.contains("analysis"));
    }

    #[test]
    fn running_bash_shows_stream_preview() {
        let leaf = ActivityLeaf {
            id: "b2".into(),
            kind: ActivityKind::Bash,
            verb: "Ran".into(),
            target: ActivityTargetKind::Command {
                command: "npm test".into(),
            },
            meta: ActivityMeta::default(),
            status: ActivityStatus::Running,
            detail_ref: Some("b2".into()),
            text: Some("[stdout] tests passed".into()),
        };
        let line = format_leaf_line(&leaf);
        assert!(line.contains("tests passed"));
    }

    #[test]
    fn running_bash_shows_spinner_prefix() {
        let leaf = ActivityLeaf {
            id: "b1".into(),
            kind: ActivityKind::Bash,
            verb: "Ran".into(),
            target: ActivityTargetKind::Command {
                command: "npm install".into(),
            },
            meta: ActivityMeta::default(),
            status: ActivityStatus::Running,
            detail_ref: Some("b1".into()),
            text: None,
        };
        let line = format_leaf_line(&leaf);
        assert!(line.starts_with('◐'));
        assert!(line.contains("npm install"));
    }

    #[test]
    fn bash_router_extracts_stdout_from_envelope() {
        let raw = r#"{"title":"Run npm test","output":{"stdout":"ok\n","stderr":"","exit_code":0},"metadata":{}}"#;
        assert!(
            crate::workspace::activity_click_router::extract_bash_display_text(raw).contains("ok")
        );
    }

    #[test]
    fn bash_router_extracts_streaming_json() {
        let raw = r#"{"stdout":"line1\nline2","stderr":"warn"}"#;
        let text = crate::workspace::activity_click_router::extract_bash_display_text(raw);
        assert!(text.contains("line1"));
        assert!(text.contains("warn"));
    }

    #[test]
    fn bash_router_strips_legacy_progress_prefix() {
        let raw = "[stdout] line1\n[stderr] warn";
        assert_eq!(
            crate::workspace::activity_click_router::extract_bash_display_text(raw),
            "line1\nwarn"
        );
    }
}
