use deepseekcode_schema::activity::DiffStats;
use gpui::prelude::FluentBuilder;
use gpui::*;
use similar::{ChangeTag, TextDiff};
use std::collections::HashSet;

use crate::file_editor_input::FileEditorInput;
use crate::syntax::{highlight_rgba, HighlightKind, ReadOnlyBuffer, SyntaxHighlighter};
use crate::ui::icon::{render_icon_rgba, IconName};

/// True for markdown source files (`.md`, `.markdown`, `.mdx`,
/// case-insensitive).
pub fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // Strip query-like suffixes just in case; compare the file name part.
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    name.ends_with(".md")
        || name.ends_with(".markdown")
        || name.ends_with(".mdx")
        || name == "readme"
}

#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub path: Option<String>,
    pub content: String,
    pub buffer: Option<ReadOnlyBuffer>,
    pub diff_hunks: Option<String>,
    pub diff_stats: Option<DiffStats>,
    pub side_by_side_diff: bool,
    pub scroll_to_line: Option<u32>,
    /// Markdown preview toggle. Off = code view (default on file open),
    /// on = rendered with the same renderer used for LLM output.
    pub md_preview: bool,
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

pub(crate) fn line_gutter_width(line_count: usize) -> Pixels {
    let digits = line_count.max(1).to_string().len().max(2);
    px((digits as f32 * 9.0 + 20.0).max(44.0))
}

fn render_line_gutter(
    line_no: usize,
    gutter_width: Pixels,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    // Structural right alignment (`justify_end`): the number always hugs the
    // right edge no matter the digit count, in both read-only and editable
    // views (which share this helper and the width formula above).
    div()
        .flex()
        .justify_end()
        .flex_shrink_0()
        .w(gutter_width)
        .px_2()
        .font_family(crate::theme::font_mono(theme))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(theme.text_disabled)
        .child(format!("{line_no}"))
}

impl EditorView {
    pub fn render(
        state: &EditorState,
        tabs: &[String],
        active_tab: usize,
        read_only: bool,
        dirty: bool,
        saving: bool,
        file_editor: Entity<FileEditorInput>,
        preview_source: &str,
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
                    .child(deepseekcode_i18n::t("editor.select_file")),
            );
        };

        let has_diff = state
            .diff_hunks
            .as_ref()
            .map(|h| !h.is_empty())
            .unwrap_or(false);
        let start_line = state
            .scroll_to_line
            .map(|l| l.saturating_sub(1) as usize)
            .unwrap_or(0);

        // Single header row: filename tabs on the left, actions right-aligned.
        // The full-path row is gone; hovering a tab shows its full path.
        panel = panel.child(render_editor_tabs(
            cx,
            tabs,
            active_tab,
            state,
            read_only,
            dirty,
            saving,
            &theme,
        ));

        let is_md = is_markdown_path(path);

        if state.side_by_side_diff && has_diff {
            return panel.child(render_side_by_side(
                state.diff_hunks.as_deref().unwrap_or_default(),
                &theme,
            ));
        }

        // Markdown preview: reuse the LLM output renderer (`render_markdown`).
        // `preview_source` is the live buffer text threaded down from the
        // workspace so edits in Code reflect immediately.
        if is_md && state.md_preview {
            return panel.child(render_md_preview(path, preview_source, &theme, cx));
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

        panel.child(
            div()
                .id("editor-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .font_family(crate::theme::font_mono(&theme))
                .child(render_readonly_source(state, path, &theme)),
        )
    }
}

/// Markdown preview body: same renderer as LLM output.
///
/// Own selection surface (`TextSurface::Editor`): drag-select works here
/// exactly like in the timeline, and surface scoping keeps the two panels
/// from ever highlighting together.
fn render_md_preview(
    path: &str,
    source: &str,
    theme: &crate::theme::ThemeTokens,
    cx: &Context<crate::workspace::WorkspaceView>,
) -> AnyElement {
    let preview = div()
        .id("editor-md-preview")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p_4()
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(
                |this, event: &MouseDownEvent, window, cx| {
                    this.text_selection_down(event.position, event.click_count, window, cx);
                },
            ),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            this.text_selection_move(event.position, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.text_selection_up(cx);
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.text_selection_up_out(cx);
            }),
        );
    if source.is_empty() {
        return preview
            .text_sm()
            .text_color(theme.text_muted)
            .child(deepseekcode_i18n::t("editor.empty_file"))
            .into_any_element();
    }
    preview
        .child(crate::markdown::render_markdown(
            source,
            theme,
            path,
            &cx.entity(),
            crate::selectable_text::TextSurface::Editor,
        ))
        .into_any_element()
}

/// Read-only source lines (shared by Code and Split left pane).
fn render_readonly_source(
    state: &EditorState,
    path: &str,
    theme: &crate::theme::ThemeTokens,
) -> AnyElement {
    let line_count = state.line_count();
    let start_line = state
        .scroll_to_line
        .map(|l| l.saturating_sub(1) as usize)
        .unwrap_or(0);
    let highlighted_lines = diff_highlighted_lines(state);
    let mut highlighter = SyntaxHighlighter::for_path(path);
    let gutter_width = line_gutter_width(line_count);

    let mut body = div().flex().flex_col().w_full();
    for idx in start_line..line_count {
        let line_no = idx + 1;
        let line = state.line_at(idx);
        let (bg, _) = line_style(line_no, &highlighted_lines, theme);
        body = body.child(
            div()
                .flex()
                .items_start()
                .flex_shrink_0()
                .text_sm()
                .line_height(px(20.))
                .bg(bg)
                .child(render_line_gutter(line_no, gutter_width, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px_2()
                        .child(render_highlighted_line(&line, highlighter.as_mut(), theme)),
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
                .child(render_line_gutter(1, gutter_width, theme))
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .text_color(theme.text_muted)
                        .child(deepseekcode_i18n::t("editor.empty_file")),
                ),
        );
    }

    body.into_any_element()
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
                .child(deepseekcode_i18n::tr!(
                    "editor.changed_lines",
                    "count" => changed_rows as u64
                )),
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
                .child(deepseekcode_i18n::t("editor.no_hunks")),
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

/// Icon-only action button with a hover name tooltip.
///
/// `active` drives the selected background so toggle state (preview on/off,
/// side-by-side on/off) stays visible without text labels.
fn icon_action_btn(
    cx: &Context<crate::workspace::WorkspaceView>,
    id: &'static str,
    icon: IconName,
    tip: String,
    active: bool,
    theme: &crate::theme::ThemeTokens,
    on_click: impl Fn(
        &mut crate::workspace::WorkspaceView,
        &MouseDownEvent,
        &mut Window,
        &mut Context<crate::workspace::WorkspaceView>,
    ) + 'static,
) -> AnyElement {
    let tip_text: SharedString = tip.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded_md()
        .cursor_pointer()
        .bg(if active {
            theme.bg_selected
        } else {
            gpui::hsla(0., 0., 0., 0.).into()
        })
        .hover(|s| s.bg(theme.bg_hover))
        .tooltip(move |_, cx| {
            cx.new(|_| crate::git_panel::GitFileTooltip {
                text: tip_text.clone(),
            })
            .into()
        })
        .on_mouse_down(MouseButton::Left, cx.listener(on_click))
        .child(render_icon_rgba(
            icon,
            px(13.),
            if active {
                theme.text_primary
            } else {
                theme.text_muted
            },
        ))
        .into_any_element()
}

fn render_editor_tabs(
    cx: &Context<crate::workspace::WorkspaceView>,
    tabs: &[String],
    active_tab: usize,
    state: &EditorState,
    read_only: bool,
    dirty: bool,
    saving: bool,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    let mut tab_list = div().flex().items_center().gap_1().min_w(px(0.));

    for (idx, path) in tabs.iter().enumerate() {
        let is_active = idx == active_tab;
        let label = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        let can_close = tabs.len() > 1;
        // Hovering the filename shows the full path (the old path row is gone).
        // NOTE: `.id()` first — `tooltip` only exists on stateful divs.
        let full_path: SharedString = path.clone().into();
        let tab_id = ElementId::Name(format!("editor-tab-{idx}").into());
        tab_list = tab_list.child(
            div()
                .id(tab_id)
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
                .tooltip(move |_, cx| {
                    cx.new(|_| crate::git_panel::GitFileTooltip {
                        text: full_path.clone(),
                    })
                    .into()
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

    // Right-aligned actions. Preview stays at the far right; Save only
    // appears next to it once the file is modified.
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
    let is_md = state
        .path
        .as_ref()
        .is_some_and(|p| is_markdown_path(p));

    let mut actions = div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .text_xs()
        .text_color(theme.text_muted);
    if !diff_label.is_empty() {
        actions = actions.child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(theme.accent_muted)
                .text_color(theme.success)
                .child(diff_label),
        );
    }
    if read_only {
        actions = actions.child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(theme.bg_hover)
                .text_color(theme.warning)
                .child(deepseekcode_i18n::t("common.read_only")),
        );
    } else if dirty {
        // P2: saving state — disabled + spinner text so rapid Ctrl+S can't
        // look dead (logic already debounces via editor_saving).
        let saving_label = saving;
        actions = actions.child(
            div()
                .id("editor-save")
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(theme.accent_muted)
                .when(!saving_label, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .tooltip(move |_, cx| {
                            cx.new(|_| crate::git_panel::GitFileTooltip {
                                text: deepseekcode_i18n::t("editor.save").into(),
                            })
                            .into()
                        })
                })
                .when(saving_label, |el| el.opacity(0.6))
                .when(!saving_label, |el| {
                    el.on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.save_editor_file(cx);
                        }),
                    )
                })
                .child(if saving_label {
                    deepseekcode_i18n::t("common.loading")
                } else {
                    deepseekcode_i18n::t("editor.save")
                }),
        );
    }
    if has_diff {
        let tip = if state.side_by_side_diff {
            deepseekcode_i18n::t("editor.unified")
        } else {
            deepseekcode_i18n::t("editor.side_by_side")
        };
        actions = actions.child(icon_action_btn(
            cx,
            "editor-diff-toggle",
            IconName::PanelRight,
            tip,
            state.side_by_side_diff,
            theme,
            |this, _, _, cx| {
                this.editor.side_by_side_diff = !this.editor.side_by_side_diff;
                cx.notify();
            },
        ));
    }
    if is_md {
        actions = actions.child(icon_action_btn(
            cx,
            "editor-preview-toggle",
            IconName::Eye,
            deepseekcode_i18n::t("editor.preview"),
            state.md_preview,
            theme,
            |this, _, _, cx| {
                this.editor.md_preview = !this.editor.md_preview;
                cx.notify();
            },
        ));
    }

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .flex_shrink_0()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border_subtle)
        .child(tab_list)
        .child(actions)
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
    deepseekcode_i18n::tr!(
        "editor.line_stats",
        "added" => added as u64,
        "removed" => removed as u64
    )
}

#[cfg(test)]
mod tests {
    use super::is_markdown_path;

    #[test]
    fn markdown_paths_are_detected_case_insensitively() {
        assert!(is_markdown_path("README.md"));
        assert!(is_markdown_path("docs/guide.MD"));
        assert!(is_markdown_path("notes.markdown"));
        assert!(is_markdown_path("CHANGELOG.mdx"));
        assert!(!is_markdown_path("main.rs"));
        assert!(!is_markdown_path("notes.md.bak"));
        assert!(!is_markdown_path("md"));
    }

    #[test]
    fn md_preview_defaults_to_off() {
        assert!(!super::EditorState::default().md_preview);
    }
}
