//! Routes activity leaf clicks to the right workspace panel.

use crate::layout::RightPanelTab;
use crate::logic::{default_todo_highlight_index, todo_index_for_leaf};
use crate::workspace::WorkspaceView;
use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};
use deepseekcode_types::SessionId;
use gpui::*;
use std::str::FromStr;

impl WorkspaceView {
    pub(crate) fn route_activity_leaf_click(
        &mut self,
        leaf_id: &str,
        kind: ActivityKind,
        target: &ActivityTargetKind,
        meta: &ActivityMeta,
        detail_ref: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if kind == ActivityKind::TodoWrite || kind == ActivityKind::TodoRead {
            self.highlight_todo_leaf(leaf_id);
        }
        if let Some(key) = detail_ref {
            self.selected_detail_key = Some(key.to_string());
        }
        match (kind, target) {
            (
                ActivityKind::Read
                | ActivityKind::Write
                | ActivityKind::Edit
                | ActivityKind::ApplyPatch,
                ActivityTargetKind::File {
                    path, line_start, ..
                },
            ) => {
                self.layout.right_panel_tab = RightPanelTab::Editor;
                self.layout.show_right_panel = true;
                self.open_file_at_line(path, *line_start, meta.diff_stats.clone(), cx);
            }
            (ActivityKind::Grep, ActivityTargetKind::Pattern { .. }) => {
                if let (Some(path), Some(line)) =
                    (meta.first_match_path.as_deref(), meta.first_match_line)
                {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_file_at_line(path, Some(line), meta.diff_stats.clone(), cx);
                } else if let Some(path) =
                    grep_fallback_path(detail_ref, &self.activity.detail_store)
                {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_file(&path, cx);
                }
            }
            (ActivityKind::Lsp, _) => {
                if let Some(path) = meta.first_match_path.as_deref() {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_file_at_line(path, None, meta.diff_stats.clone(), cx);
                } else {
                    self.layout.show_right_panel = true;
                }
            }
            (ActivityKind::Skill, ActivityTargetKind::Skill { name }) => {
                self.layout.right_panel_tab = RightPanelTab::Editor;
                self.layout.show_right_panel = true;
                if let Some(path) = meta.first_match_path.as_deref() {
                    let skill_doc = format!("{path}/{name}/SKILL.md");
                    self.open_file(&skill_doc, cx);
                }
            }
            (
                ActivityKind::Mcp,
                ActivityTargetKind::File {
                    path, line_start, ..
                },
            ) => {
                self.layout.right_panel_tab = RightPanelTab::Editor;
                self.layout.show_right_panel = true;
                self.open_file_at_line(path, *line_start, meta.diff_stats.clone(), cx);
            }
            (ActivityKind::Mcp, ActivityTargetKind::Url { url }) => {
                self.layout.show_right_panel = true;
                self.set_web_preview(url, detail_ref);
            }
            (ActivityKind::Mcp, ActivityTargetKind::Generic { label }) => {
                if label.starts_with("http://") || label.starts_with("https://") {
                    self.layout.show_right_panel = true;
                    self.set_web_preview(label, detail_ref);
                } else if let Some(path) = meta.first_match_path.as_deref() {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_file_at_line(
                        path,
                        meta.first_match_line,
                        meta.diff_stats.clone(),
                        cx,
                    );
                } else if detail_ref.is_some() {
                    self.layout.show_right_panel = true;
                }
            }
            (ActivityKind::Mcp, _) => {
                if let Some(path) = meta.first_match_path.as_deref() {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_file_at_line(
                        path,
                        meta.first_match_line,
                        meta.diff_stats.clone(),
                        cx,
                    );
                } else if detail_ref.is_some() {
                    self.layout.show_right_panel = true;
                }
            }
            (ActivityKind::Bash, ActivityTargetKind::Command { .. }) => {
                self.layout.right_panel_tab = RightPanelTab::Terminal;
                self.layout.show_right_panel = true;
                if let Some(key) = detail_ref {
                    self.show_bash_output_in_terminal(key, cx);
                }
            }
            (ActivityKind::TodoWrite | ActivityKind::TodoRead, ActivityTargetKind::TodoList) => {
                self.composer_todo_expanded = true;
            }
            (ActivityKind::Task, ActivityTargetKind::Session { session_id, .. }) => {
                if let Ok(sid) = SessionId::from_str(session_id) {
                    self.switch_session_by_id(sid, cx);
                    return;
                }
            }
            (ActivityKind::WebFetch, ActivityTargetKind::Url { url })
            | (ActivityKind::WebSearch, ActivityTargetKind::Url { url }) => {
                self.layout.show_right_panel = true;
                self.set_web_preview(url, detail_ref);
            }
            (ActivityKind::WebSearch, ActivityTargetKind::Generic { .. }) => {
                self.layout.show_right_panel = true;
            }
            (ActivityKind::Glob | ActivityKind::List, ActivityTargetKind::Pattern { .. }) => {
                self.layout.right_panel_tab = RightPanelTab::Files;
                self.layout.show_right_panel = true;
                if let Some(path) = meta.first_match_path.as_deref() {
                    self.reveal_path_in_file_tree(path, cx);
                } else if let Some(path) =
                    glob_list_fallback_path(detail_ref, &self.activity.detail_store)
                {
                    self.reveal_path_in_file_tree(&path, cx);
                }
            }
            (ActivityKind::Question, _) => {
                self.layout.show_right_panel = true;
            }
            (ActivityKind::PlanReady, _) => {
                if let Some(path) = meta.first_match_path.as_deref() {
                    self.layout.right_panel_tab = RightPanelTab::Editor;
                    self.layout.show_right_panel = true;
                    self.open_plan_file(path, cx);
                }
            }
            (ActivityKind::Permission, _) => {
                self.policy_reject_expanded = false;
                if let Some(id) = detail_ref {
                    self.focus_policy_prompt(id);
                }
            }
            _ => {
                if detail_ref.is_some() {
                    self.layout.show_right_panel = true;
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn highlight_todo_leaf(&mut self, leaf_id: &str) {
        self.highlighted_todo_leaf_id = Some(leaf_id.to_string());
        self.highlighted_todo_index = todo_index_for_leaf(&self.activity.activity_tree, leaf_id)
            .or_else(|| default_todo_highlight_index(&self.session_todos));
    }

    pub(crate) fn reveal_path_in_file_tree(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(project_dir) = self.project_dir.clone() else {
            return;
        };
        let normalized = path.replace('\\', "/");
        let full = if std::path::Path::new(path).is_absolute() {
            normalized
        } else {
            project_dir.join(path).to_string_lossy().replace('\\', "/")
        };
        let root = project_dir.to_string_lossy().replace('\\', "/");
        let mut dir = std::path::Path::new(&full)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| root.clone());
        while dir.len() >= root.len() {
            if !self.file_tree.is_expanded(&dir) {
                self.file_tree.expanded.push(dir.clone());
                if !self.file_tree.entries.contains_key(&dir) {
                    self.load_directory(&dir, cx);
                }
            }
            if dir == root {
                break;
            }
            dir = std::path::Path::new(&dir)
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| root.clone());
        }
        self.layout.right_panel_tab = RightPanelTab::Files;
        self.layout.show_right_panel = true;
        cx.notify();
    }

    pub(crate) fn refresh_bash_terminal_output(
        &mut self,
        detail_key: &str,
        cx: &mut Context<Self>,
    ) {
        let display = self
            .activity
            .detail_store
            .get(detail_key)
            .and_then(|d| d.output.as_deref())
            .map(extract_bash_display_text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                self.activity
                    .detail_store
                    .get(detail_key)
                    .and_then(|d| d.input.as_deref())
                    .map(bash_running_terminal_text)
                    .unwrap_or_else(|| "◐ Running…".to_string())
            });

        if !self
            .terminals
            .iter()
            .any(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        {
            return;
        }
        if let Some(tab_idx) = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        {
            if let Some(tab) = self.terminals.get_mut(tab_idx) {
                tab.state.output = display.clone().into();
                if let Ok(mut buf) = tab.buffer.try_lock() {
                    *buf = display;
                }
            }
            if self.active_terminal == tab_idx {
                cx.notify();
            }
        }
    }

    pub(crate) fn show_bash_output_in_terminal(
        &mut self,
        detail_key: &str,
        _cx: &mut Context<Self>,
    ) {
        self.bash_output_detail_ref = Some(detail_key.to_string());
        let command = self
            .activity
            .detail_store
            .get(detail_key)
            .and_then(|d| d.input.as_deref())
            .unwrap_or("(unknown command)");
        let output = self
            .activity
            .detail_store
            .get(detail_key)
            .and_then(|d| d.output.clone())
            .unwrap_or_default();
        let display = extract_bash_display_text(&output);
        let display = if display.is_empty() {
            bash_running_terminal_text(command)
        } else {
            display
        };
        let tab_idx = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
            .unwrap_or_else(|| {
                let id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let title = format!("Bash #{}", id);
                self.terminals
                    .push(crate::terminal::TerminalTab::new_bash_output(
                        id, title, detail_key,
                    ));
                self.terminals.len() - 1
            });
        if let Some(tab) = self.terminals.get_mut(tab_idx) {
            tab.state.connected = false;
            tab.state.pty_connecting = false;
            tab.state.connect_error = None;
            tab.state.pty_id = None;
            tab.state.input_tx = None;
            tab.state.output = display.clone().into();
            if let Ok(mut buf) = tab.buffer.try_lock() {
                *buf = display;
            }
        }
        self.active_terminal = tab_idx;
        self.layout.right_panel_tab = crate::layout::RightPanelTab::Terminal;
        self.layout.show_right_panel = true;
    }

    fn set_web_preview(&mut self, url: &str, detail_ref: Option<&str>) {
        let preview = detail_ref
            .and_then(|k| self.activity.detail_store.get(k))
            .and_then(|d| d.web_preview.clone().or_else(|| d.output.clone()));
        if let Some(key) = detail_ref {
            self.activity
                .detail_store
                .set_web_preview(key, url, preview);
        }
    }
}

fn grep_fallback_path(
    detail_ref: Option<&str>,
    store: &crate::activity_timeline::DetailStore,
) -> Option<String> {
    let output = detail_ref.and_then(|k| store.get(k))?.output.as_ref()?;
    let v = serde_json::from_str::<serde_json::Value>(output).ok()?;
    v.get("output")
        .and_then(|o| o.get("matches"))
        .and_then(|m| m.get(0))
        .and_then(|m| m.get("path"))
        .and_then(|p| p.as_str())
        .map(String::from)
}

fn glob_list_fallback_path(
    detail_ref: Option<&str>,
    store: &crate::activity_timeline::DetailStore,
) -> Option<String> {
    let output = detail_ref.and_then(|k| store.get(k))?.output.as_ref()?;
    let v = serde_json::from_str::<serde_json::Value>(output).ok()?;
    let body = v.get("output").unwrap_or(&v);
    body.get("path")
        .and_then(|p| p.as_str())
        .or_else(|| {
            body.get("entries")
                .and_then(|e| e.as_array())
                .and_then(|a| a.first())
                .and_then(|e| e.get("path"))
                .and_then(|p| p.as_str())
        })
        .or_else(|| {
            body.get("files")
                .and_then(|f| f.as_array())
                .and_then(|a| a.first())
                .and_then(|p| p.as_str())
        })
        .map(String::from)
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct BashStreamBody {
    stdout: String,
    stderr: String,
}

pub(crate) fn merge_bash_completed_display(
    streamed: Option<&str>,
    completed_value: &serde_json::Value,
) -> String {
    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(raw) = streamed {
        if let Ok(body) = serde_json::from_str::<BashStreamBody>(raw) {
            stdout = body.stdout;
            stderr = body.stderr;
        } else {
            let partial = extract_bash_display_text(raw);
            if !partial.is_empty() {
                stdout = partial;
            }
        }
    }

    let out = completed_value
        .get("output")
        .filter(|v| !v.is_null())
        .unwrap_or(completed_value);
    if let Some(s) = out.get("stdout").and_then(|v| v.as_str()) {
        if stdout.is_empty() {
            stdout = s.to_string();
        }
    }
    if let Some(s) = out.get("stderr").and_then(|v| v.as_str()) {
        if stderr.is_empty() {
            stderr = s.to_string();
        }
    }

    let mut display = extract_bash_display_text(
        &serde_json::json!({ "stdout": stdout, "stderr": stderr }).to_string(),
    );

    if let Some(err) = out.get("error").and_then(|v| v.as_str()) {
        if display.is_empty() {
            display = format!("Error: {err}");
        } else {
            display.push_str(&format!("\n\nError: {err}"));
        }
    }

    if display.is_empty() {
        display = extract_bash_display_text(
            &serde_json::to_string(completed_value).unwrap_or_else(|_| completed_value.to_string()),
        );
    }

    display
}

pub(crate) fn bash_running_terminal_text(command: &str) -> String {
    format!("$ {command}\n\n◐ Running… (output streams here)")
}

pub(crate) fn extract_bash_display_text(output: &str) -> String {
    if let Ok(stream) = serde_json::from_str::<BashStreamBody>(output) {
        if stream.stderr.is_empty() {
            return stream.stdout;
        }
        if stream.stdout.is_empty() {
            return stream.stderr;
        }
        return format!("{}\n{}", stream.stdout, stream.stderr);
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(output) {
        let body = v.get("output").unwrap_or(&v);
        if let Some(stdout) = body.get("stdout").and_then(|s| s.as_str()) {
            let stderr = body.get("stderr").and_then(|s| s.as_str()).unwrap_or("");
            if stderr.is_empty() {
                return stdout.to_string();
            }
            return format!("{stdout}\n{stderr}");
        }
        if let Some(content) = body.get("content").and_then(|s| s.as_str()) {
            return content.to_string();
        }
    }
    output
        .lines()
        .map(|line| {
            line.strip_prefix("[stdout] ")
                .or_else(|| line.strip_prefix("[stderr] "))
                .unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{extract_bash_display_text, glob_list_fallback_path, grep_fallback_path};
    use crate::logic::{plan_activity_click_route, ActivityClickRoute};
    use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};

    #[test]
    fn merge_bash_completed_display_preserves_stream_and_error() {
        let streamed = r#"{"stdout":"partial\n","stderr":""}"#;
        let completed = serde_json::json!({
            "title": "bash failed",
            "output": { "error": "command timed out after 60000ms", "success": false }
        });
        let merged = super::merge_bash_completed_display(Some(streamed), &completed);
        assert!(merged.contains("partial"));
        assert!(merged.contains("command timed out"));
    }

    #[test]
    fn extract_bash_display_text_prefers_stream_body() {
        let raw = r#"{"stdout":"ok\n","stderr":""}"#;
        assert_eq!(extract_bash_display_text(raw), "ok\n");
    }

    #[test]
    fn grep_fallback_path_reads_first_match() {
        let mut store = crate::activity_timeline::DetailStore::default();
        store.insert(
            "g1",
            crate::activity_timeline::DetailPayload {
                output: Some(r#"{"output":{"matches":[{"path":"src/a.rs","line":3}]}}"#.into()),
                ..Default::default()
            },
        );
        assert_eq!(
            grep_fallback_path(Some("g1"), &store).as_deref(),
            Some("src/a.rs")
        );
    }

    #[test]
    fn glob_list_fallback_path_reads_first_entry() {
        let mut store = crate::activity_timeline::DetailStore::default();
        store.insert(
            "l1",
            crate::activity_timeline::DetailPayload {
                output: Some(r#"{"output":{"entries":[{"path":"src"}]}}"#.into()),
                ..Default::default()
            },
        );
        assert_eq!(
            glob_list_fallback_path(Some("l1"), &store).as_deref(),
            Some("src")
        );
    }

    #[test]
    fn plan_route_task_switches_session() {
        let route = plan_activity_click_route(
            ActivityKind::Task,
            &ActivityTargetKind::Session {
                session_id: "child-1".into(),
                label: Some("build".into()),
            },
            &ActivityMeta::default(),
            None,
        );
        assert_eq!(
            route,
            ActivityClickRoute::SwitchSession {
                session_id: "child-1".into()
            }
        );
    }
}
