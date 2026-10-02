use crate::terminal_vt::{ColoredLine, TerminalGrid};
use crate::ui::multiline_text::split_display_lines;
use crate::ui::scrollbar::render_flex_scroll_viewport;
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
            connect_error: None,
            pty_id: None,
            input_tx: None,
        }
    }
}

fn terminal_output_text(tab: &TerminalTab) -> String {
    placeholder_for_tab(tab)
}

fn terminal_status_text(tab: Option<&TerminalTab>) -> &'static str {
    let Some(tab) = tab else {
        return "no terminal";
    };
    if tab.bash_detail_key.is_some() {
        "command output"
    } else if tab.state.connected {
        "connected"
    } else if tab.state.pty_connecting {
        "connecting…"
    } else if tab.state.connect_error.is_some() {
        "disconnected"
    } else {
        "idle"
    }
}

fn placeholder_for_tab(tab: &TerminalTab) -> String {
    if tab.bash_detail_key.is_some() {
        if tab.state.output_received {
            if tab.state.output.is_empty() {
                // Ran to completion and genuinely printed nothing.
                return "(command produced no output)".to_string();
            }
            return tab.state.output.to_string();
        }
        return "◐ Running… (waiting for output)".to_string();
    }

    if let Some(err) = &tab.state.connect_error {
        return format!(
            "PTY connection failed: {err}\n\nClose this tab and open a new Shell (+) to retry."
        );
    }
    if !tab.state.output.is_empty() {
        return tab.state.output.to_string();
    }
    if tab.state.pty_connecting {
        return "Connecting to shell…".to_string();
    }
    if tab.state.connected {
        return String::new();
    }
    "Shell not connected — switch to Terminal or click + for a new shell".to_string()
}

fn terminal_line_style(theme: &crate::theme::ThemeTokens, el: Div) -> Div {
    el.text_size(px(13.))
        .font_family(crate::theme::font_mono(theme))
        .text_color(theme.terminal_fg)
        .line_height(px(20.))
        .whitespace_nowrap()
}

fn render_plain_terminal_lines(theme: &crate::theme::ThemeTokens, text: &str) -> impl IntoElement {
    let mut column = terminal_line_style(theme, div().flex().flex_col().w_full());
    if text.is_empty() {
        return column.child(terminal_line_style(theme, div()).h(px(20.)));
    }
    for line in split_display_lines(text) {
        column = column.child(terminal_line_style(theme, div()).child(line));
    }
    column
}

fn render_colored_terminal_lines(
    theme: &crate::theme::ThemeTokens,
    lines: &[ColoredLine],
) -> impl IntoElement {
    let mono = crate::theme::font_mono(theme);
    let mut column = div().flex().flex_col().w_full();
    for line in lines {
        let mut row = div()
            .flex()
            .flex_row()
            .w_full()
            .line_height(px(20.))
            .whitespace_nowrap();
        if line.spans.is_empty() {
            row = row.child(div().h(px(20.)));
        } else {
            for span in &line.spans {
                row = row.child(
                    div()
                        .text_size(px(13.))
                        .font_family(mono.clone())
                        .text_color(rgb(span.fg.0))
                        .line_height(px(20.))
                        .whitespace_nowrap()
                        .child(span.text.clone()),
                );
            }
        }
        column = column.child(row);
    }
    column
}

fn render_terminal_body(
    theme: &crate::theme::ThemeTokens,
    tab: Option<&TerminalTab>,
) -> impl IntoElement {
    let Some(tab) = tab else {
        return render_plain_terminal_lines(theme, "$ no terminal").into_any_element();
    };

    if tab.bash_detail_key.is_some() {
        return render_plain_terminal_lines(theme, &terminal_output_text(tab)).into_any_element();
    }

    if let Ok(grid) = tab.grid.try_lock() {
        let colored = grid.render_colored_lines();
        if !colored.is_empty() {
            return render_colored_terminal_lines(theme, &colored).into_any_element();
        }
    }

    render_plain_terminal_lines(theme, &terminal_output_text(tab)).into_any_element()
}

/// One PTY tab — buffer/grid are shared with the async WS reader task.
#[derive(Clone)]
pub struct TerminalTab {
    pub id: usize,
    pub title: SharedString,
    pub state: TerminalState,
    pub buffer: Arc<Mutex<String>>,
    pub grid: Arc<Mutex<TerminalGrid>>,
    /// When set, this tab shows read-only bash tool output for the given detail key.
    pub bash_detail_key: Option<String>,
}

impl TerminalTab {
    pub fn new(id: usize, title: impl Into<SharedString>) -> Self {
        Self {
            id,
            title: title.into(),
            state: TerminalState::default(),
            buffer: Arc::new(Mutex::new(String::new())),
            grid: Arc::new(Mutex::new(TerminalGrid::new())),
            bash_detail_key: None,
        }
    }

    pub fn new_bash_output(
        id: usize,
        title: impl Into<SharedString>,
        detail_key: impl Into<String>,
    ) -> Self {
        Self {
            bash_detail_key: Some(detail_key.into()),
            ..Self::new(id, title)
        }
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
                .child("No terminal open"),
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
                    cx.listener(|this, _, _, cx| {
                        this.add_terminal_tab(cx);
                    }),
                )
                .child("+ New Shell"),
        )
}

impl TerminalPanel {
    pub fn render(
        cx: &Context<crate::workspace::WorkspaceView>,
        tabs: &[TerminalTab],
        active: usize,
        terminal_focus: &FocusHandle,
        scroll_handle: &ScrollHandle,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);

        if tabs.is_empty() {
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

        let active_tab = tabs.get(active);
        let status = terminal_status_text(active_tab);

        let mut tab_bar = div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_shrink_0()
            .w(px(112.))
            .px_1()
            .py_2()
            .border_r_1()
            .border_color(theme.border_subtle)
            .bg(theme.bg_surface);

        for (idx, tab) in tabs.iter().enumerate() {
            let is_active = idx == active;
            tab_bar = tab_bar.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .bg(if is_active {
                        theme.bg_selected
                    } else {
                        theme.bg_base
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let idx = idx;
                        cx.listener(move |this, _, _, cx| {
                            this.switch_terminal_tab(idx, cx);
                        })
                    })
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(tab.title.to_string()),
                    )
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let idx = idx;
                                cx.listener(move |this, _, _, cx| {
                                    this.close_terminal_tab(idx, cx);
                                    cx.stop_propagation();
                                })
                            })
                            .child("×"),
                    ),
            );
        }

        tab_bar = tab_bar.child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .bg(theme.bg_base)
                .text_color(theme.text_muted)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.add_terminal_tab(cx);
                    }),
                )
                .child("+"),
        );

        div()
            .id("terminal")
            .h_full()
            .w_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .bg(theme.terminal_bg)
            .border_t_1()
            .border_color(theme.border_default)
            .key_context("Terminal")
            .track_focus(terminal_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.send_terminal_input(&event.keystroke, cx);
            }))
            .child(tab_bar)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .flex_shrink_0()
                            .px_2()
                            .py_0p5()
                            .border_b_1()
                            .border_color(theme.border_subtle)
                            .bg(theme.bg_surface)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_disabled)
                                    .child(format!("{status}")),
                            ),
                    )
                    .child(render_flex_scroll_viewport(
                        cx,
                        scroll_handle,
                        "terminal-output",
                        "terminal-scrollbar",
                        false,
                        div()
                            .px_3()
                            .py_2()
                            .bg(theme.terminal_bg)
                            .child(render_terminal_body(&theme, active_tab)),
                    )),
            )
    }
}
