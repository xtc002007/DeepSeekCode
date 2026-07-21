use deepseekcode_schema::activity::DiffStats;
use gpui::*;
use similar::{ChangeTag, TextDiff};
use std::collections::HashSet;

use crate::file_editor_input::FileEditorInput;
use crate::syntax::{highlight_rgba, HighlightKind, ReadOnlyBuffer, SyntaxHighlighter};

#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub path: Option<String>,
    pub content: String,
    pub buffer: Option<ReadOnlyBuffer>,
    pub diff_hunks: Option<String>,
    pub diff_stats: Option<DiffStats>,
    pub side_by_side_diff: bool,
    pub scroll_to_line: Option<u32>,
}

impl EditorState {
    pub fn set_content(&mut self, content: String) {
        self.buffer = Some(ReadOnlyBuffer::from_str(&content));
        self.content = content;
    }

    fn line_count(&self) -> usize {
        self.buffer
            .as_ref()
            .map(|b| b.line_count())
            .unwrap_or_else(|| self.content.lines().count().max(1))
    }

    fn line_at(&self, idx: usize) -> String {
        self.buffer
            .as_ref()
            .map(|b| b.line(idx))
            .unwrap_or_else(|| {
                self.content
                    .lines()
                    .nth(idx)
                    .unwrap_or_default()
                    .to_string()
            })
    }
}

pub struct EditorView;

fn line_gutter_width(line_count: usize) -> Pixels {
    let digits = line_count.max(1).to_string().len().max(2);
    px((digits as f32 * 9.0 + 20.0).max(44.0))
}

fn render_line_gutter(
    line_no: usize,
    gutter_width: Pixels,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .w(gutter_width)
        .px_2()
        .font_family(crate::theme::font_mono(theme))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(theme.text_disabled)
        .text_align(TextAlign::Right)
        .child(format!("{line_no}"))
}

impl EditorView {
    pub fn render(
        state: &EditorState,
        tabs: &[String],
        active_tab: usize,
        read_only: bool,
        file_editor: Entity<FileEditorInput>,
        cx: &Context<crate::workspace::WorkspaceView>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let mut panel = div()
            .id("editor")
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_hidden()
            .bg(theme.bg_base)
            .border_b_1()
            .border_color(theme.border_default);

        let Some(path) = &state.path else {
            return panel.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(theme.text_disabled)
                    .child("Select a file from the tree to view"),
            );
        };

        let has_diff = state
            .diff_hunks
            .as_ref()
            .map(|h| !h.is_empty())
            .unwrap_or(false);
        let diff_label = state
            .diff_stats
            .as_ref()
            .map(|d| format!("+{} -{}", d.added, d.removed))
            .or_else(|| {
                state
                    .diff_hunks
                    .as_ref()
                    .map(|d| diff_stats_label(d.as_str()))
            })
            .unwrap_or_default();
        let line_count = state.line_count();
        let start_line = state
            .scroll_to_line
            .map(|l| l.saturating_sub(1) as usize)
            .unwrap_or(0);

        panel = panel.child(render_editor_tabs(cx, tabs, active_tab, &theme));

        panel = panel.child(
            div()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(theme.text_muted)
                .border_b_1()
                .border_color(theme.border_subtle)
                .child(path.clone())
                .children({
                    let mut actions = vec![];
                    if !diff_label.is_empty() {
                        actions.push(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(theme.accent_muted)
                                .text_color(theme.success)
                                .child(diff_label)
                                .into_any_element(),
                        );
                    }
                    if read_only {
                        actions.push(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(theme.bg_hover)
                                .text_color(theme.warning)
                                .child("Read-only")
                                .into_any_element(),
                        );
                    } else {
                        actions.push(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.accent_muted)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.save_editor_file(cx);
                                    }),
                                )
                                .child("Save")
                                .into_any_element(),
                        );
                    }
                    if has_diff {
                        actions.push(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if state.side_by_side_diff {
                                    theme.bg_selected
                                } else {
                                    theme.bg_elevated
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.editor.side_by_side_diff =
                                            !this.editor.side_by_side_diff;
                                        cx.notify();
                                    }),
                                )
                                .child(if state.side_by_side_diff {
                                    "Unified diff"
                                } else {
                                    "Side-by-side"
                                })
                                .into_any_element(),
                        );
                    }
                    actions
                }),
        );

        if state.side_by_side_diff && has_diff {
            return panel.child(render_side_by_side(
                state.diff_hunks.as_deref().unwrap_or_default(),
                &theme,
            ));
        }

        if !read_only {
            let top_pad = if state.scroll_to_line.is_some() {
                px((start_line.saturating_mul(20)) as f32)
            } else {
                px(0.)
            };
            return panel.child(
                div()
                    .id("editor-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .child(div().h(top_pad))
                            .child(file_editor.clone()),
                    ),
            );
        }

        let highlighted_lines = diff_highlighted_lines(state);
        let mut highlighter = SyntaxHighlighter::for_path(path);
        let gutter_width = line_gutter_width(line_count);

        let mut body = div()
            .id("editor-body")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .font_family(crate::theme::font_mono(&theme));

        for idx in start_line..line_count {
            let line_no = idx + 1;
            let line = state.line_at(idx);
            let (bg, _) = line_style(line_no, &highlighted_lines, &theme);
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .flex_shrink_0()
                    .text_sm()
                    .line_height(px(20.))
                    .bg(bg)
                    .child(render_line_gutter(line_no, gutter_width, &theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px_2()
                            .child(render_highlighted_line(&line, highlighter.as_mut(), &theme)),
                    ),
            );
        }

        if line_count == 1 && state.content.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .flex_shrink_0()
                    .text_sm()
                    .line_height(px(20.))
                    .child(render_line_gutter(1, gutter_width, &theme))
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .text_color(theme.text_muted)
                            .child("(empty file)"),
                    ),
            );
        }

        panel.child(body)
    }
}

fn render_highlighted_line(
    line: &str,
    highlighter: Option<&mut SyntaxHighlighter>,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let default_color = theme.syntax.default;
    let Some(highlighter) = highlighter else {
        return div().text_color(default_color).child(line.to_string());
    };

    let spans = highlighter.highlight_line(line);
    if spans.is_empty() {
        return div().text_color(default_color).child(line.to_string());
    }

    let mut row = div().flex().flex_wrap();
    let mut cursor = 0usize;
    for (start, end, kind) in spans {
        if start > cursor {
            row = row.child(
                div()
                    .text_color(highlight_rgba(HighlightKind::Default, theme))
                    .child(line[cursor..start].to_string()),
            );
        }
        if end > start {
            row = row.child(
                div()
                    .text_color(highlight_rgba(kind, theme))
                    .child(line[start..end.min(line.len())].to_string()),
            );
        }
        cursor = end.max(cursor);
    }
    if cursor < line.len() {
        row = row.child(
            div()
                .text_color(highlight_rgba(HighlightKind::Default, theme))
                .child(line[cursor..].to_string()),
        );
    }
    row
}

#[derive(Clone, Debug)]
struct DiffRow {
    left: Option<String>,
    right: Option<String>,
    kind: ChangeTag,
}

fn parse_unified_diff(hunks: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for line in hunks.lines() {
        if line.starts_with("@@") || line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            rows.push(DiffRow {
                left: None,
                right: Some(text.to_string()),
                kind: ChangeTag::Insert,
            });
        } else if let Some(text) = line.strip_prefix('-') {
            rows.push(DiffRow {
                left: Some(text.to_string()),
                right: None,
                kind: ChangeTag::Delete,
            });
        } else if let Some(text) = line.strip_prefix(' ') {
            rows.push(DiffRow {
                left: Some(text.to_string()),
                right: Some(text.to_string()),
                kind: ChangeTag::Equal,
            });
        }
    }
    rows
}

fn render_side_by_side(hunks: &str, theme: &crate::theme::ThemeTokens) -> impl IntoElement {
    let rows = parse_unified_diff(hunks);
    let changed_rows = crate::logic::parse_unified_diff_rows(hunks);
    let mut body = div()
        .id("editor-side-by-side")
        .flex_1()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .font_family(crate::theme::font_mono(&theme))
        .text_xs()
        .child(
            div()
                .px_2()
                .py_1()
                .text_color(theme.text_muted)
                .child(format!("{changed_rows} changed lines")),
        );

    for row in rows.iter() {
        let (left_bg, right_bg) = match row.kind {
            ChangeTag::Delete => (theme.diff.delete_side_bg, hsla(0.0, 0.0, 0.0, 0.0)),
            ChangeTag::Insert => (hsla(0.0, 0.0, 0.0, 0.0), theme.diff.insert_side_bg),
            ChangeTag::Equal => (hsla(0.0, 0.0, 0.0, 0.0), hsla(0.0, 0.0, 0.0, 0.0)),
        };
        body = body.child(
            div()
                .flex()
                .border_b_1()
                .border_color(theme.border_subtle)
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_0p5()
                        .bg(left_bg)
                        .text_color(theme.text_primary)
                        .child(row.left.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_0p5()
                        .bg(right_bg)
                        .text_color(theme.text_primary)
                        .child(row.right.clone().unwrap_or_default()),
                ),
        );
    }

    if rows.is_empty() {
        body = body.child(
            div()
                .p_4()
                .text_color(theme.text_disabled)
                .child("No diff hunks to display"),
        );
    }

    body
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Normal,
    Added,
    Removed,
}

fn diff_highlighted_lines(state: &EditorState) -> Vec<LineKind> {
    let line_count = state.line_count();
    let Some(hunks) = &state.diff_hunks else {
        return vec![LineKind::Normal; line_count];
    };

    if hunks.is_empty() {
        return vec![LineKind::Normal; line_count];
    }

    let mut added = HashSet::new();
    let mut removed = HashSet::new();
    let mut current_line = 0usize;

    for line in hunks.lines() {
        if line.starts_with("@@") {
            if let Some(rest) = line.split(' ').nth(2) {
                if let Some(num) = rest.trim_start_matches('+').split(',').next() {
                    if let Ok(n) = num.parse::<usize>() {
                        current_line = n.saturating_sub(1);
                    }
                }
            }
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            added.insert(current_line);
            current_line += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed.insert(current_line);
        } else if line.starts_with(' ') {
            current_line += 1;
        }
    }

    let line_count = state.line_count();
    (0..line_count)
        .map(|i| {
            if added.contains(&i) {
                LineKind::Added
            } else if removed.contains(&i) {
                LineKind::Removed
            } else {
                LineKind::Normal
            }
        })
        .collect()
}

fn line_style(
    line_no: usize,
    kinds: &[LineKind],
    theme: &crate::theme::ThemeTokens,
) -> (Hsla, Hsla) {
    let idx = line_no.saturating_sub(1);
    match kinds.get(idx) {
        Some(LineKind::Added) => (theme.diff.insert_bg, theme.diff.insert_gutter),
        Some(LineKind::Removed) => (theme.diff.delete_bg, theme.diff.delete_gutter),
        _ => (hsla(0.0, 0.0, 0.0, 0.0), theme.diff.equal_gutter),
    }
}

fn render_editor_tabs(
    cx: &Context<crate::workspace::WorkspaceView>,
    tabs: &[String],
    active_tab: usize,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border_subtle);

    for (idx, path) in tabs.iter().enumerate() {
        let is_active = idx == active_tab;
        let label = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        let can_close = tabs.len() > 1;
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
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
                        this.switch_editor_tab(idx, cx);
                    })
                })
                .child(label.to_string())
                .children(if can_close {
                    vec![div()
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, {
                            let idx = idx;
                            cx.listener(move |this, _, _, cx| {
                                this.close_editor_tab(idx, cx);
                                cx.stop_propagation();
                            })
                        })
                        .child("×")
                        .into_any_element()]
                } else {
                    vec![]
                }),
        );
    }

    row
}

pub fn compute_local_diff(original: &str, current: &str) -> String {
    let _tags = change_tags(original, current);
    let diff = TextDiff::from_lines(original, current);
    diff.unified_diff().context_radius(3).to_string()
}

pub fn change_tags(original: &str, current: &str) -> Vec<ChangeTag> {
    TextDiff::from_lines(original, current)
        .iter_all_changes()
        .map(|c| c.tag())
        .collect()
}

fn diff_stats_label(diff: &str) -> String {
    let mut added = 0u32;
    let mut removed = 0u32;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    if added == 0 && removed == 0 {
        return String::new();
    }
    format!("+{added} -{removed}")
}
