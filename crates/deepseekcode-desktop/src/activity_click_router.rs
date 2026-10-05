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

    /// Ensure at least one interactive (non-bash) shell tab exists.
    ///
    /// Bash transcript tabs are read-only — their processes already exited —
    /// so a Terminal page holding only transcripts leaves the user with
    /// nowhere to type. This appends one unconnected shell (no focus steal;
    /// it connects eagerly here and lazily again on activation) and is
    /// idempotent.
    pub(crate) fn ensure_live_shell_tab(&mut self, cx: &mut Context<Self>) {
        if self
            .terminals
            .iter()
            .any(|t| !t.is_bash() && t.session_id == self.session_id)
        {
            return;
        }
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let title = deepseekcode_i18n::tr!(
            "terminal.shell_named",
            "name" => (self.terminals.len() + 1) as u64
        );
        let session_id = self.session_id;
        self.terminals
            .push(crate::terminal::TerminalTab::new(id, title, session_id));
        let idx = self.terminals.len() - 1;
        self.connect_terminal_tab(idx, cx);
        cx.notify();
    }

    /// Ensure a terminal tab exists for the given bash `tool_call_id`.
    ///
    /// Called on `ToolStarted` so every LLM-created execution is findable in
    /// the Terminal page even before the user ever clicks its activity leaf.
    /// Unlike `show_bash_output_in_terminal` this does NOT steal focus.
    /// It also guarantees a live shell tab next to the transcripts so the
    /// user always has somewhere to type.
    pub(crate) fn ensure_bash_terminal_tab(
        &mut self,
        detail_key: &str,
        cx: &mut Context<Self>,
        session_id: Option<deepseekcode_types::SessionId>,
    ) -> usize {
        let tab_idx = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
            .unwrap_or_else(|| {
                let id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let detail = self.activity.detail_store.get(detail_key);
                let command = detail.and_then(|d| d.command.clone());
                let title = bash_tab_title(id, command.as_deref());
                self.terminals
                    .push(crate::terminal::TerminalTab::new_bash_output(
                        id, title, detail_key, session_id,
                    ));
                self.terminals.len() - 1
            });
        // Late-bound ownership: a tab recreated (e.g. click-to-reopen after
        // auto-close) joins the session being viewed.
        if let Some(tab) = self.terminals.get_mut(tab_idx) {
            if tab.session_id != session_id {
                tab.session_id = session_id;
            }
        }
        self.sync_bash_tab_state(tab_idx, detail_key);
        self.ensure_live_shell_tab(cx);
        tab_idx
    }

    /// Mirror the session-scoped detail entry into the long-lived transcript
    /// store. Called on every Started/Progress/Completed and on tab ensure:
    /// later session switches rebuild `detail_store` from scratch, but the
    /// global terminal tabs (and this mirror) survive, so transcripts never
    /// degrade into `$ (unknown command)`. Never clears a known command.
    pub(crate) fn upsert_bash_transcript_from_detail(&mut self, detail_key: &str) {
        let Some(detail) = self.activity.detail_store.get(detail_key) else {
            return;
        };
        let entry = self
            .bash_transcripts
            .entry(detail_key.to_string())
            .or_default();
        if let Some(cmd) = detail.command.clone().filter(|c| !c.trim().is_empty()) {
            entry.command = cmd;
        }
        if detail.input.is_some() {
            entry.input = detail.input.clone();
        }
        if let Some(out) = detail.output.clone() {
            entry.output = out;
            entry.received = true;
        }
        if detail.title.is_some() {
            entry.title = detail.title.clone();
        }
        if entry.cwd.is_empty() {
            if let Some(dir) = self.project_dir.as_ref() {
                entry.cwd = dir.display().to_string();
            }
        }
    }

    fn sync_bash_tab_state(&mut self, tab_idx: usize, detail_key: &str) {
        self.upsert_bash_transcript_from_detail(detail_key);
        let transcript = self.bash_transcripts.get(detail_key);
        let command = transcript
            .and_then(|t| {
                crate::terminal_vscode::transcript_command(
                    Some(t.command.as_str()).filter(|c| !c.trim().is_empty()),
                    t.input.as_deref(),
                )
            })
            .or_else(|| {
                // No transcript yet (e.g. tab restored before any event):
                // fall back to the live detail entry.
                let detail = self.activity.detail_store.get(detail_key);
                crate::terminal_vscode::transcript_command(
                    detail.and_then(|d| d.command.as_deref()),
                    detail.and_then(|d| d.input.as_deref()),
                )
            });
        // Tri-state: real output present, real-but-empty, or still pending.
        // Store the RAW payload (JSON when available) — the renderer parses
        // it into transcript rows via `parse_bash_result`.
        let (raw, received) = match transcript {
            Some(t) if t.received => (t.output.clone(), true),
            _ => match self
                .activity
                .detail_store
                .get(detail_key)
                .and_then(|d| d.output.clone())
            {
                Some(out) => (out, true),
                None => (String::new(), false),
            },
        };
        if let Some(tab) = self.terminals.get_mut(tab_idx) {
            tab.state.connected = false;
            tab.state.pty_connecting = false;
            tab.state.connect_error = None;
            tab.state.pty_id = None;
            tab.state.input_tx = None;
            tab.state.output = raw.clone().into();
            tab.state.output_received = received;
            // Keep the tab findable: `Bash #7 · cargo test`.
            if let Some(cmd) = command {
                let short = truncate_command(&cmd, 32);
                if !short.is_empty() && !tab.title.contains(short.as_str()) {
                    let base =
                        deepseekcode_i18n::tr!("terminal.bash_title", "id" => tab.id as u64);
                    tab.title = format!("{base} · {short}").into();
                }
            }
            if let Ok(mut buf) = tab.buffer.try_lock() {
                *buf = raw;
            }
        }
    }

    /// Ensure every bash execution in the detail store has a terminal tab.
    ///
    /// Used after session hydration / SSE gap backfill so history executions
    /// are all listed in the Terminal page (transcripts stay read-only; the
    /// shared helper also guarantees a live shell next to them).
    pub(crate) fn sync_all_bash_terminal_tabs(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self
            .activity
            .detail_store
            .bash_keys()
            .into_iter()
            .map(str::to_string)
            .collect();
        let session_id = self.session_id;
        for key in keys {
            // Completed commands leave no tab behind (they auto-close on
            // completion); only still-running ones get a live tab. Either
            // way the transcript is materialised so click-to-reopen works.
            let completed = self
                .activity
                .detail_store
                .get(&key)
                .and_then(|d| d.output.as_ref())
                .is_some();
            if completed {
                self.upsert_bash_transcript_from_detail(&key);
                continue;
            }
            // `ensure` is idempotent — existing tabs are only refreshed.
            self.ensure_bash_terminal_tab(&key, cx, session_id);
        }
        // Fresh project with no history yet still gets a usable shell, so the
        // Terminal page is never an empty dead end.
        self.ensure_live_shell_tab(cx);
        self.fix_active_terminal_visible();
        cx.notify();
    }

    pub(crate) fn refresh_bash_terminal_output(
        &mut self,
        detail_key: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_idx) = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        else {
            return;
        };
        self.sync_bash_tab_state(tab_idx, detail_key);
        // Notify when the terminal is visible (active tab or Terminal panel
        // open): the tab strip dot (running → done) must update while the
        // user watches another terminal tab. Otherwise skip to keep streamed
        // `ToolProgress` cheap (the outer SSE loop already throttles).
        if self.active_terminal == tab_idx
            || self.layout.right_panel_tab == crate::layout::RightPanelTab::Terminal
        {
            cx.notify();
        }
    }

    /// Open a finished run as a NORMAL terminal: a fresh interactive shell
    /// with the recorded echo + output pre-rendered into its grid, fully
    /// typable and focusable afterwards (repeat clicks re-activate instead
    /// of spawning duplicates). A still-running command keeps its live tab.
    pub(crate) fn show_bash_output_in_terminal(
        &mut self,
        detail_key: &str,
        cx: &mut Context<Self>,
    ) {
        self.bash_output_detail_ref = Some(detail_key.to_string());
        let session_id = self.session_id;
        // 1. Live (or placeholder) tab for this call still around? Use it.
        // (tool_call_ids are globally unique, so the found tab is ours.)
        if let Some(vec_idx) = self
            .terminals
            .iter()
            .position(|t| t.bash_detail_key.as_deref() == Some(detail_key))
        {
            self.active_terminal = vec_idx;
            self.layout.right_panel_tab = crate::layout::RightPanelTab::Terminal;
            self.layout.show_right_panel = true;
            tracing::info!(
                target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                detail_key = %detail_key,
                tab_id = self.terminals.get(vec_idx).map(|t| t.id).unwrap_or(0),
                "bash leaf opened live tab"
            );
            cx.notify();
            return;
        }
        // 2. Replay shell already opened for it? Re-activate.
        if let Some(vec_idx) = self
            .terminals
            .iter()
            .position(|t| t.replay_detail_key.as_deref() == Some(detail_key))
        {
            self.active_terminal = vec_idx;
            self.layout.right_panel_tab = crate::layout::RightPanelTab::Terminal;
            self.layout.show_right_panel = true;
            cx.notify();
            return;
        }
        // 3. Otherwise spawn a fresh interactive shell with history replay.
        self.upsert_bash_transcript_from_detail(detail_key);
        let transcript = self.bash_transcripts.get(detail_key).cloned();
        let (command, stdout, stderr, cwd) = match &transcript {
            Some(t) => {
                let parsed = crate::terminal_vscode::parse_bash_result(&t.output);
                let cmd = crate::terminal_vscode::transcript_command(
                    Some(t.command.as_str()).filter(|c| !c.trim().is_empty()),
                    t.input.as_deref(),
                )
                .unwrap_or_else(|| deepseekcode_i18n::t("terminal.unknown_command").to_string());
                (cmd, parsed.stdout, parsed.stderr, t.cwd.clone())
            }
            None => (
                deepseekcode_i18n::t("terminal.unknown_command").to_string(),
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let title = bash_tab_title(id, Some(command.as_str()));
        let replay =
            crate::terminal_vscode::build_replay_bytes(&cwd, &command, &stdout, &stderr);
        let has_command = transcript.as_ref().is_some_and(|t| {
            !t.command.trim().is_empty()
                || t.input.as_deref().is_some_and(|s| !s.trim().is_empty())
        });
        tracing::info!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            detail_key = %detail_key,
            tab_id = id,
            has_command,
            output_bytes = stdout.len() + stderr.len(),
            "bash leaf opened replay shell"
        );
        self.terminals.push(crate::terminal::TerminalTab::new_replay(
            id, title, detail_key, replay, session_id,
        ));
        self.active_terminal = self.terminals.len().saturating_sub(1);
        self.layout.right_panel_tab = crate::layout::RightPanelTab::Terminal;
        self.layout.show_right_panel = true;
        self.connect_terminal_tab(self.active_terminal, cx);
        cx.notify();
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
        let suffix = deepseekcode_i18n::tr!("terminal.error_prefix", "error" => err);
        if display.is_empty() {
            display = suffix;
        } else {
            display.push_str("\n\n");
            display.push_str(&suffix);
        }
    }

    if display.is_empty() {
        display = extract_bash_display_text(
            &serde_json::to_string(completed_value).unwrap_or_else(|_| completed_value.to_string()),
        );
    }

    display
}

/// Tab title for a bash execution: `Bash #7 · cargo test`.
/// The command suffix is what makes every LLM execution findable in the
/// Terminal page without opening its activity leaf first.
pub(crate) fn bash_tab_title(id: usize, command: Option<&str>) -> String {
    let base = deepseekcode_i18n::tr!("terminal.bash_title", "id" => id as u64);
    match command.map(truncate_command_for_title) {
        Some(short) if !short.is_empty() => format!("{base} · {short}"),
        _ => base.to_string(),
    }
}

fn truncate_command_for_title(command: &str) -> String {
    truncate_command(command, 32)
}

/// First line of the command, collapsed whitespace, capped at `max` chars.
pub(crate) fn truncate_command(command: &str, max: usize) -> String {
    let first = command.lines().next().unwrap_or("").trim();
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
}

/// Shell shown before any output has arrived.
pub(crate) fn bash_pending_body(command: &str) -> String {
    deepseekcode_i18n::tr!("terminal.bash_pending", "command" => command)
}

/// Shell body once output is known: the echoed command, then the output.
/// An empty result is stated explicitly instead of implying it is still running.
pub(crate) fn bash_output_body(command: &str, output: &str) -> String {
    if output.trim().is_empty() {
        deepseekcode_i18n::tr!("terminal.bash_empty", "command" => command)
    } else {
        format!("$ {command}\n\n{output}")
    }
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
    use super::{
        bash_output_body, bash_pending_body, bash_tab_title, extract_bash_display_text,
        glob_list_fallback_path, grep_fallback_path, truncate_command,
    };
    use crate::logic::{plan_activity_click_route, ActivityClickRoute};
    use deepseekcode_schema::activity::{ActivityKind, ActivityMeta, ActivityTargetKind};

    /// 截图问题：终端把 `{"command": …, "timeout_ms": …}` 整个 JSON 当成
    /// `$` 提示行打印。命令必须单独取出，JSON 只留在详情面板。
    #[test]
    fn terminal_body_echoes_command_not_input_json() {
        let command = "cd /d D:\\Workspace\\Echo && pnpm install";
        let body = bash_output_body(command, "added 412 packages");
        let first = body.lines().next().unwrap();
        assert_eq!(first, format!("$ {command}"));
        assert!(!body.contains("timeout_ms"), "json leaked: {body}");
        assert!(body.contains("added 412 packages"), "{body}");
    }

    #[test]
    fn terminal_body_distinguishes_empty_result_from_pending() {
        // 空输出 = 已完成且无输出；pending = 还没收到任何输出。二者必须可区分，
        // 否则永远停在 "Running…" 会让人以为命令还在跑。
        assert!(bash_output_body("echo hi", "").contains("no output"));
        assert!(bash_pending_body("echo hi").contains("Running"));
        assert!(!bash_pending_body("echo hi").contains("no output"));
    }

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
    fn bash_tab_title_carries_command_for_findability() {
        // 终端页必须一眼找到每条 LLM 执行：`Bash #7 · cargo test`。
        let title = bash_tab_title(7, Some("cargo test -p deepseekcode-desktop"));
        assert!(title.contains("Bash"), "{title}");
        assert!(title.contains("cargo test"), "{title}");
        let bare = bash_tab_title(7, None);
        assert!(!bare.contains('·'), "{bare}");
    }

    #[test]
    fn truncate_command_keeps_first_line_and_collapses_whitespace() {
        assert_eq!(truncate_command("  cargo   test  ", 32), "cargo test");
        assert_eq!(
            truncate_command("echo hi\nsecond line", 32),
            "echo hi"
        );
        let long = "a".repeat(100);
        let short = truncate_command(&long, 32);
        assert!(short.ends_with('…'), "{short}");
        assert!(short.chars().count() <= 32, "{short}");
    }

    #[test]
    fn bash_keys_lists_only_command_executions() {
        let mut store = crate::activity_timeline::DetailStore::default();
        store.insert(
            "bash-1",
            crate::activity_timeline::DetailPayload {
                command: Some("dir".into()),
                ..Default::default()
            },
        );
        store.insert(
            "read-1",
            crate::activity_timeline::DetailPayload {
                input: Some("{}".into()),
                ..Default::default()
            },
        );
        let keys = store.bash_keys();
        assert_eq!(keys, vec!["bash-1"]);
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
