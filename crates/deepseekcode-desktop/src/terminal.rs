use crate::terminal_vscode::{parse_bash_result, BashResult};
use crate::terminal_vt::{
    terminal_color_from_rgba, terminal_default_fg, ColoredLine, ColoredSpan, TerminalGrid,
};
use crate::ui::multiline_text::split_display_lines;
use gpui::*;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct TerminalState {
    pub output: SharedString,
    /// True once real command output has been observed for this tab.
    ///
    /// Distinguishes "still running, nothing yet" from "finished and printed
    /// nothing". Without it an empty `output` was indistinguishable from a
    /// pending one, so a command that legitimately produced no output kept
    /// rendering `◐ Running…` forever.
    pub output_received: bool,
    pub connected: bool,
    /// True while a PTY WebSocket handshake is in progress.
    pub pty_connecting: bool,
    /// True once the first live PTY frame arrived. Agent tabs render the
    /// transcript fallback until this flips (covers the attach window).
    pub pty_live: bool,
    pub connect_error: Option<SharedString>,
    pub pty_id: Option<String>,
    pub input_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            output: SharedString::default(),
            output_received: false,
            connected: false,
            pty_connecting: false,
            pty_live: false,
            connect_error: None,
            pty_id: None,
            input_tx: None,
        }
    }
}

fn terminal_output_text(tab: &TerminalTab) -> String {
    placeholder_for_tab(tab)
}

fn terminal_status_text(tab: Option<&TerminalTab>) -> String {
    let Some(tab) = tab else {
        return deepseekcode_i18n::t("terminal.none");
    };
    if tab.bash_detail_key.is_some() {
        if !tab.state.output_received {
            return deepseekcode_i18n::t("terminal.connecting");
        }
        // Derive success/failed from the stored JSON when available.
        let parsed = parse_bash_result(&tab.state.output);
        if let Some(false) = parsed.success {
            return deepseekcode_i18n::t("terminal.disconnected");
        }
        return deepseekcode_i18n::t("terminal.command_output");
    } else if tab.state.connected {
        deepseekcode_i18n::t("terminal.connected")
    } else if tab.state.pty_connecting {
        deepseekcode_i18n::t("terminal.connecting")
    } else if tab.state.connect_error.is_some() {
        deepseekcode_i18n::t("terminal.disconnected")
    } else {
        deepseekcode_i18n::t("terminal.idle")
    }
}

fn placeholder_for_tab(tab: &TerminalTab) -> String {
    if tab.bash_detail_key.is_some() {
        if tab.state.output_received {
            if tab.state.output.is_empty() {
                // Ran to completion and genuinely printed nothing.
                return deepseekcode_i18n::t("terminal.no_output");
            }
            return tab.state.output.to_string();
        }
        return deepseekcode_i18n::t("terminal.running_waiting");
    }

    if let Some(err) = &tab.state.connect_error {
        return deepseekcode_i18n::tr!(
            "terminal.connect_failed",
            "error" => err.as_str()
        );
    }
    if !tab.state.output.is_empty() {
        return tab.state.output.to_string();
    }
    if tab.state.pty_connecting {
        return deepseekcode_i18n::t("terminal.connecting_shell");
    }
    if tab.state.connected {
        return String::new();
    }
    deepseekcode_i18n::t("terminal.not_connected")
}

// ---------------------------------------------------------------------------
// VSCode-aligned line rendering.
//
// The previous implementation used `w_full` flex rows: a terminal line wider
// than the right panel was squeezed into the container width and GPUI broke
// words mid-glyph (`Name` → `Nam`/`e`, `.github` → three lines — see the
// issue screenshot). VSCode instead keeps every terminal row on ONE visual
// line and offers a horizontal scrollbar. Every row below is therefore
// `flex_shrink_0` + `whitespace_nowrap` inside an `overflow_x_scroll`
// viewport, never `w_full`.
// ---------------------------------------------------------------------------

const TERMINAL_FONT_SIZE: f32 = 13.0;
const TERMINAL_LINE_HEIGHT: f32 = 19.0;

fn terminal_line_style(theme: &crate::theme::ThemeTokens, el: Div) -> Div {
    el.text_size(px(TERMINAL_FONT_SIZE))
        .font_family(crate::theme::font_mono(theme))
        .text_color(theme.terminal_fg)
        .line_height(px(TERMINAL_LINE_HEIGHT))
        .whitespace_nowrap()
        .flex_shrink_0()
}

fn render_plain_terminal_lines(
    theme: &crate::theme::ThemeTokens,
    text: &str,
) -> impl IntoElement {
    // NOTE: no `w_full` — see module comment. Each line is its own
    // non-shrinking row so wide tables (e.g. `dir`) scroll horizontally
    // instead of splitting `Name` into `Nam`/`e`.
    let mut column = div().flex().flex_col().flex_shrink_0().min_w_full();
    if text.is_empty() {
        return column.child(terminal_line_style(theme, div()).h(px(TERMINAL_LINE_HEIGHT)));
    }
    for line in split_display_lines(text) {
        // Empty lines still occupy one terminal row (VSCode behaviour).
        let body = if line.is_empty() { " ".to_string() } else { line };
        column = column.child(terminal_line_style(theme, div()).child(body));
    }
    column
}

/// Resolve a VT foreground against the active theme.
///
/// The shell emits no color for plain output, which alacritty reports as the
/// palette `Foreground` default (`0xCCCCCC`). Rendering that verbatim is
/// correct on a dark background but nearly invisible on a light one, so the
/// default maps to `terminal_fg` (dark `#1E1E1E` bg → `#CCC`, white bg →
/// `#333`) while explicit ANSI colors pass through untouched.
fn span_fg(theme: &crate::theme::ThemeTokens, span: &ColoredSpan) -> Rgba {
    if span.fg == terminal_default_fg() {
        theme.terminal_fg
    } else {
        rgb(span.fg.0)
    }
}

fn render_colored_terminal_lines(
    theme: &crate::theme::ThemeTokens,
    lines: &[ColoredLine],
) -> impl IntoElement {
    let mono = crate::theme::font_mono(theme);
    let mut column = div().flex().flex_col().flex_shrink_0().min_w_full();
    for line in lines {
        let mut row = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .line_height(px(TERMINAL_LINE_HEIGHT))
            .whitespace_nowrap();
        if line.spans.is_empty() {
            row = row.child(div().h(px(TERMINAL_LINE_HEIGHT)).w(px(8.)));
        } else {
            for span in &line.spans {
                let mut cell = div()
                    .text_size(px(TERMINAL_FONT_SIZE))
                    .font_family(mono.clone())
                    .text_color(span_fg(theme, span))
                    .line_height(px(TERMINAL_LINE_HEIGHT))
                    .whitespace_nowrap()
                    .flex_shrink_0();
                if let Some(bg) = span.bg {
                    cell = cell.bg(rgb(bg.0));
                }
                if span.bold {
                    cell = cell.font_weight(FontWeight::BOLD);
                }
                if span.dim {
                    cell = cell.opacity(0.7);
                }
                cell = cell.child(span.text.clone());
                row = row.child(cell);
            }
        }
        column = column.child(row);
    }
    column
}

/// Read-only bash tool transcript, rendered like a real console session:
///
/// ```text
/// D:\Workspace\Echoo> dir /b packages
/// .bin
/// typescript
/// ```
///
/// One plain `{cwd}> {command}` echo line (no highlighting — exactly what the
/// OS shell itself prints) plus the raw stdout/stderr rows in the shared
/// monospace style, inside the same horizontal-scroll viewport the live
/// shell uses. The backing process has already exited, so the tab is
/// read-only; `+` opens a live shell.
fn render_bash_detail_body(
    theme: &crate::theme::ThemeTokens,
    tab: &TerminalTab,
    transcript: Option<&crate::activity_timeline::BashTranscript>,
    cx: &Context<crate::workspace::WorkspaceView>,
) -> AnyElement {
    // Session switches rebuild `detail_store`, but transcripts are global —
    // the tab keeps its command/output either way (never `(unknown command)`
    // while we ever saw the real one).
    let command = transcript
        .and_then(|t| {
            crate::terminal_vscode::transcript_command(
                Some(t.command.as_str()).filter(|c| !c.trim().is_empty()),
                t.input.as_deref(),
            )
        })
        .unwrap_or_else(|| deepseekcode_i18n::t("terminal.unknown_command").to_string());
    let raw_output = transcript
        .map(|t| t.output.clone())
        .filter(|o| !o.is_empty())
        .unwrap_or_else(|| tab.state.output.to_string());
    let parsed: BashResult = parse_bash_result(&raw_output);
    let running = !tab.state.output_received;

    let mut column = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .min_w_full()
        .px_3()
        .py_2();

    // Echo line exactly as the OS shell prints it: `{cwd}> {command}`.
    let echo = transcript
        .map(|t| t.cwd.trim())
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| format!("{cwd}> {command}"))
        .unwrap_or_else(|| format!("$ {command}"));
    column = column.child(terminal_line_style(theme, div()).child(echo));

    if running && parsed.is_empty() {
        column = column.child(
            terminal_line_style(theme, div())
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::t("terminal.running_waiting").to_string()),
        );
    } else {
        if !parsed.stdout.is_empty() {
            column = column.child(render_plain_terminal_lines(theme, &parsed.stdout));
        }
        if !parsed.stderr.is_empty() {
            // stderr in terminal red — kept on its own rows so tables never
            // interleave mid-line.
            let mut err_col = div().flex().flex_col().flex_shrink_0().min_w_full();
            for line in split_display_lines(&parsed.stderr) {
                let body = if line.is_empty() { " ".to_string() } else { line };
                err_col = err_col.child(
                    div()
                        .text_size(px(TERMINAL_FONT_SIZE))
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(theme.error)
                        .line_height(px(TERMINAL_LINE_HEIGHT))
                        .whitespace_nowrap()
                        .flex_shrink_0()
                        .child(body),
                );
            }
            column = column.child(err_col);
        }
        if let Some(err) = parsed.error.as_ref().filter(|e| !e.trim().is_empty()) {
            // Avoid double-printing the error when it is already the whole
            // stderr body.
            if parsed.stderr.trim() != err.trim().trim_start_matches("Error: ") {
                column = column.child(
                    div()
                        .text_size(px(TERMINAL_FONT_SIZE))
                        .font_family(crate::theme::font_mono(theme))
                        .text_color(theme.error)
                        .line_height(px(TERMINAL_LINE_HEIGHT))
                        .whitespace_nowrap()
                        .flex_shrink_0()
                        .child(err.clone()),
                );
            }
        }
        if !running && parsed.is_empty() {
            column = column.child(
                terminal_line_style(theme, div())
                    .text_color(theme.text_muted)
                    .child(deepseekcode_i18n::t("terminal.no_output").to_string()),
            );
        }
    }

    // Failure footer only — a successful run stays clean like a real terminal.
    // (Truncation is still noted so missing output is never silent.)
    let failed = parsed.success == Some(false)
        || parsed.exit_code.is_some_and(|c| c != 0)
        || parsed.error.is_some();
    let mut footer_parts: Vec<String> = Vec::new();
    if failed {
        if let Some(code) = parsed.exit_code {
            footer_parts.push(
                deepseekcode_i18n::tr!("terminal.exit_code", "code" => code).to_string(),
            );
        }
    }
    if parsed.truncated {
        footer_parts.push(deepseekcode_i18n::t("terminal.truncated").to_string());
    }
    if !footer_parts.is_empty() {
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.text_disabled)
                .whitespace_nowrap()
                .flex_shrink_0()
                .child(footer_parts.join(" · ")),
        );
    }
    // This tab is read-only (process gone, or attach never happened): always
    // offer the door into a live, typable shell right here, so a dead
    // transcript never strands the user with nowhere to type.
    column = column.child(
        div()
            .mt_2()
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .flex_shrink_0()
            .border_1()
            .border_color(theme.border_default)
            .text_color(theme.text_muted)
            .hover(|s| s.bg(theme.bg_hover).text_color(theme.text_primary))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.add_terminal_tab(cx);
                    this.focus_terminal(window, cx);
                }),
            )
            .child(deepseekcode_i18n::t("terminal.new_shell")),
    );
    column.into_any_element()
}

fn render_pty_body(
    theme: &crate::theme::ThemeTokens,
    tab: &TerminalTab,
    show_cursor: bool,
) -> AnyElement {
    // The block cursor belongs at the end of the last row (right after the
    // prompt), never on a row of its own at the far left.
    let with_cursor = show_cursor && tab.state.connected;
    if let Ok(grid) = tab.grid.try_lock() {
        let mut colored = grid.render_colored_lines();
        if !colored.is_empty() {
            if with_cursor {
                let cursor = ColoredSpan {
                    text: "▊".to_string(),
                    fg: terminal_color_from_rgba(theme.terminal_cursor),
                    bg: None,
                    bold: false,
                    dim: false,
                };
                if let Some(last) = colored.last_mut() {
                    last.spans.push(cursor);
                } else {
                    colored.push(crate::terminal_vt::ColoredLine {
                        spans: vec![cursor],
                    });
                }
            }
            return render_colored_terminal_lines(theme, &colored).into_any_element();
        }
    }
    let mut text = terminal_output_text(tab);
    if with_cursor {
        // `render_text` never emits a trailing newline, so pushing the glyph
        // lands it exactly at the end of the prompt row.
        if text.is_empty() {
            text = "▊".to_string();
        } else {
            text.push('▊');
        }
    }
    render_plain_terminal_lines(theme, &text).into_any_element()
}

fn render_terminal_body(
    theme: &crate::theme::ThemeTokens,
    tab: Option<&TerminalTab>,
    transcript: Option<&crate::activity_timeline::BashTranscript>,
    is_active: bool,
    cx: &Context<crate::workspace::WorkspaceView>,
) -> AnyElement {
    let Some(tab) = tab else {
        return render_plain_terminal_lines(
            theme,
            &deepseekcode_i18n::t("terminal.prompt_fallback"),
        )
        .into_any_element();
    };

    if tab.is_live_agent() {
        // Real attached terminal: grid once live, transcript fallback while
        // the WebSocket handshake is still in flight.
        if tab.state.pty_live {
            return render_pty_body(theme, tab, is_active);
        }
        if tab.bash_detail_key.is_some() {
            return render_bash_detail_body(theme, tab, transcript, cx);
        }
        return render_pty_body(theme, tab, false);
    }
    if tab.bash_detail_key.is_some() {
        return render_bash_detail_body(theme, tab, transcript, cx);
    }
    render_pty_body(theme, tab, is_active)
}

/// One PTY tab — buffer/grid are shared with the async WS reader task.
///
/// Tabs belong to exactly one session (`session_id`): the Terminal page only
/// ever lists the current session's tabs, so switching sessions never shows
/// another session's leftovers.
#[derive(Clone)]
pub struct TerminalTab {
    pub id: usize,
    pub title: SharedString,
    pub state: TerminalState,
    pub buffer: Arc<Mutex<String>>,
    pub grid: Arc<Mutex<TerminalGrid>>,
    /// When set, this tab shows read-only bash tool output for the given detail key.
    pub bash_detail_key: Option<String>,
    /// When set, the tab is LIVE-attached to an agent PTY (same grid/input
    /// pipeline as hand-opened shells): the running command's real terminal.
    pub agent_pty_id: Option<String>,
    /// History replay: clicking a FINISHED run opens a fresh interactive
    /// shell with these bytes pre-rendered into its grid (echo + recorded
    /// output), then stays fully typable. Linked back to the detail key so
    /// repeat clicks activate instead of spawning duplicates.
    pub replay_detail_key: Option<String>,
    pub pending_replay: Option<Vec<u8>>,
    /// Owning session. Tabs from other sessions are hidden, never destroyed.
    pub session_id: Option<deepseekcode_types::SessionId>,
}

impl TerminalTab {
    pub fn new(
        id: usize,
        title: impl Into<SharedString>,
        session_id: Option<deepseekcode_types::SessionId>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            state: TerminalState::default(),
            buffer: Arc::new(Mutex::new(String::new())),
            grid: Arc::new(Mutex::new(TerminalGrid::new())),
            bash_detail_key: None,
            agent_pty_id: None,
            replay_detail_key: None,
            pending_replay: None,
            session_id,
        }
    }

    pub fn new_bash_output(
        id: usize,
        title: impl Into<SharedString>,
        detail_key: impl Into<String>,
        session_id: Option<deepseekcode_types::SessionId>,
    ) -> Self {
        Self {
            bash_detail_key: Some(detail_key.into()),
            ..Self::new(id, title, session_id)
        }
    }

    /// True while attached to a live agent PTY (input reaches the running
    /// command; output streams from the real terminal).
    pub fn is_live_agent(&self) -> bool {
        self.agent_pty_id.is_some()
    }

    /// A history replay shell: a REAL interactive tab (no transcript key, so
    /// all shell paths treat it like a hand-opened one) pre-loaded with the
    /// finished run's echo + output, linked for click-to-activate dedup.
    pub fn new_replay(
        id: usize,
        title: impl Into<SharedString>,
        detail_key: impl Into<String>,
        replay: Vec<u8>,
        session_id: Option<deepseekcode_types::SessionId>,
    ) -> Self {
        Self {
            replay_detail_key: Some(detail_key.into()),
            pending_replay: Some(replay),
            ..Self::new(id, title, session_id)
        }
    }

    pub fn is_bash(&self) -> bool {
        self.bash_detail_key.is_some()
    }
}

pub struct TerminalPanel;

fn render_empty_terminal(
    theme: &crate::theme::ThemeTokens,
    cx: &Context<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .bg(theme.terminal_bg)
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(deepseekcode_i18n::t("terminal.no_terminal_open")),
        )
        .child(
            div()
                .px_3()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .bg(theme.bg_selected)
                .text_color(theme.text_primary)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.add_terminal_tab(cx);
                        this.focus_terminal(window, cx);
                    }),
                )
                .child(deepseekcode_i18n::t("terminal.new_shell")),
        )
}

/// Short tab title: `Bash #12 · cargo test` / `Shell 1` / `powershell`.
pub fn tab_display_title(tab: &TerminalTab) -> String {
    tab.title.to_string()
}

impl TerminalPanel {
    pub fn render(
        cx: &Context<crate::workspace::WorkspaceView>,
        tabs: &[TerminalTab],
        active: usize,
        current_session: Option<deepseekcode_types::SessionId>,
        terminal_focus: &FocusHandle,
        scroll_handle: &ScrollHandle,
        transcripts: &std::collections::HashMap<String, crate::activity_timeline::BashTranscript>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        // Session scoping: only this session's tabs exist here. Callbacks
        // below therefore carry *visible* positions, resolved by the caller
        // back to vec indices.
        let visible: Vec<usize> = tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.session_id == current_session)
            .map(|(idx, _)| idx)
            .collect();
        if visible.is_empty() {
            return div()
                .id("terminal")
                .h_full()
                .w_full()
                .flex()
                .flex_col()
                .bg(theme.terminal_bg)
                .border_t_1()
                .border_color(theme.border_default)
                .child(render_empty_terminal(&theme, cx));
        }
        let active_pos = visible
            .iter()
            .position(|&vec_idx| vec_idx == active)
            .unwrap_or(0);
        let active_vec = visible[active_pos];

        let active_tab = tabs.get(active_vec);
        let status = terminal_status_text(active_tab);
        let active_transcript = active_tab
            .and_then(|t| t.bash_detail_key.as_deref())
            .and_then(|k| transcripts.get(k));

        // --- Vertical tab rail (equal-width rows) ---------------------------
        // Narrow on purpose: the output keeps the remaining width. Titles
        // carry the running command (`Bash #2 · dir /b …`); overlong titles
        // ellipsize to a single line and hovering reveals the full command
        // via tooltip. Rows are `w_full` so every tab is the same width no
        // matter how long its title is.
        let mut tab_rail = div()
            .id("terminal-tab-rail")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(104.))
            .py_1()
            .px_1()
            .gap_0p5()
            .bg(theme.bg_surface)
            .border_r_1()
            .border_color(theme.border_subtle)
            .overflow_y_scroll();

        for (pos, &vec_idx) in visible.iter().enumerate() {
            let tab = &tabs[vec_idx];
            let is_active = pos == active_pos;
            let dot_color = if tab.is_bash() {
                if !tab.state.output_received {
                    theme.warning
                } else {
                    theme.tool_bash
                }
            } else if tab.state.connected {
                theme.success
            } else {
                theme.text_disabled
            };
            let title = tab_display_title(tab);
            let full_title: SharedString = title.clone().into();
            tab_rail = tab_rail.child(
                div()
                    .id(ElementId::Name(format!("terminal-tab-{}", tab.id).into()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .w_full()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_active {
                        theme.terminal_bg
                    } else {
                        theme.bg_surface
                    })
                    .text_color(if is_active {
                        theme.text_primary
                    } else {
                        theme.text_muted
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| TerminalTabTooltip {
                            text: full_title.clone(),
                        })
                        .into()
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let pos = pos;
                        cx.listener(move |this, _, window, cx| {
                            this.switch_terminal_tab(pos, cx);
                            this.focus_terminal(window, cx);
                        })
                    })
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(dot_color)
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_xs()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme.text_disabled)
                            .cursor_pointer()
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(theme.bg_hover))
                            .on_mouse_down(MouseButton::Left, {
                                let pos = pos;
                                cx.listener(move |this, _, window, cx| {
                                    this.close_terminal_tab(pos, cx);
                                    this.focus_terminal(window, cx);
                                    cx.stop_propagation();
                                })
                            })
                            .child("×"),
                    ),
            );
        }

        tab_rail = tab_rail.child(
            div()
                .w_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(theme.text_muted)
                .hover(|s| s.bg(theme.bg_hover).text_color(theme.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.add_terminal_tab(cx);
                        this.focus_terminal(window, cx);
                    }),
                )
                .child("+"),
        );

        // --- Body: single scroll container with BOTH axes ------------------
        // Vertical scroll is tracked (thin custom scrollbar on the right);
        // horizontal scroll is native so wide `dir` tables never split.
        let body_content =
            render_terminal_body(&theme, active_tab, active_transcript, true, cx);

        div()
            .id("terminal")
            .h_full()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.terminal_bg)
            .border_t_1()
            .border_color(theme.border_default)
            .key_context("Terminal")
            .track_focus(terminal_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.send_terminal_input(&event.keystroke, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .w_full()
                    .overflow_hidden()
                    .child(tab_rail)
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .min_w(px(0.))
                            .w_full()
                            .relative()
                            .child(
                                div()
                                    .id("terminal-output")
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .right(px(6.))
                                    .track_scroll(scroll_handle)
                                    .overflow_y_scroll()
                                    .overflow_x_scroll()
                                    .bg(theme.terminal_bg)
                                    .px_3()
                                    .py_2()
                                    .child(body_content),
                            )
                            .child(crate::ui::scrollbar::render_vertical_scrollbar(
                                cx,
                                scroll_handle,
                                "terminal-scrollbar",
                                false,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .flex_shrink_0()
                    .w_full()
                    .px_3()
                    .py_0p5()
                    .border_t_1()
                    .border_color(theme.border_subtle)
                    .bg(theme.bg_surface)
                    .child(
                        div()
                            .text_xs()
                            .font_family(crate::theme::font_mono(&theme))
                            .text_color(theme.text_muted)
                            .whitespace_nowrap()
                            .flex_shrink_0()
                            .child(
                                active_tab
                                    .map(|t| t.title.to_string())
                                    .unwrap_or_default(),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_disabled)
                            .whitespace_nowrap()
                            .flex_shrink_0()
                            .child(status),
                    ),
            )
    }
}

/// Hover tooltip for a terminal tab row: the full (untruncated) title, i.e.
/// the complete command for `Bash #N` tabs. Shared with the sidebar, whose
/// session rows truncate the same way.
pub(crate) struct TerminalTabTooltip {
    pub(crate) text: SharedString,
}

impl Render for TerminalTabTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_default)
            .text_xs()
            .font_family(crate::theme::font_mono(&theme))
            .text_color(theme.text_primary)
            .child(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::span_fg;
    use crate::terminal_vt::{terminal_default_fg, ColoredSpan, TerminalColor};

    /// 回归：shell 不带颜色输出时 alacritty 上报调色板 Foreground 默认值。
    /// 原样渲染在深色底上是对的，在浅色底上几乎看不见（截图全灰），
    /// 所以默认值必须映射到主题 `terminal_fg`，显式 ANSI 色才直通。
    #[test]
    fn default_foreground_follows_theme() {
        let plain = ColoredSpan {
            text: "dir".into(),
            fg: terminal_default_fg(),
            bg: None,
            bold: false,
            dim: false,
        };
        let dark = crate::theme::themes::dark::dark_tokens();
        let light = crate::theme::themes::light::light_tokens();
        assert_eq!(span_fg(&dark, &plain), dark.terminal_fg);
        assert_eq!(span_fg(&light, &plain), light.terminal_fg);
        assert_ne!(dark.terminal_fg, light.terminal_fg);

        let red = ColoredSpan {
            text: "err".into(),
            fg: TerminalColor(0xC50F1F),
            bg: None,
            bold: false,
            dim: false,
        };
        assert_eq!(span_fg(&dark, &red), gpui::rgb(0xC50F1F));
        assert_eq!(span_fg(&light, &red), gpui::rgb(0xC50F1F));
    }
}
