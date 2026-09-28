use crate::activity_rows::{flatten_activity_tree, FlattenedActivityRow};
use crate::selectable_text::{CopyTimelineSelection, TapCallback};
use deepseekcode_schema::activity::{ActivityGroup, ActivityGroupKind, ActivityNode, ActivityTree};
use deepseekcode_schema::todo::TodoItem;
use std::rc::Rc;
use std::str::FromStr;

#[derive(Clone, Debug, Default)]
pub struct DetailPayload {
    pub input: Option<String>,
    pub output: Option<String>,
    pub todos: Option<Vec<TodoItem>>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub web_preview: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DetailStore {
    entries: std::collections::HashMap<String, DetailPayload>,
}

impl DetailStore {
    pub fn insert(&mut self, key: impl Into<String>, payload: DetailPayload) {
        self.entries.insert(key.into(), payload);
    }

    pub fn get(&self, key: &str) -> Option<&DetailPayload> {
        self.entries.get(key)
    }

    pub fn append_output(&mut self, key: &str, chunk: &str, stream: &str) {
        let entry = self.entries.entry(key.to_string()).or_default();
        let mut body = parse_or_init_bash_stream(entry.output.as_deref());
        if stream == "stderr" {
            body.stderr.push_str(chunk);
            if !chunk.ends_with('\n') {
                body.stderr.push('\n');
            }
        } else {
            body.stdout.push_str(chunk);
            if !chunk.ends_with('\n') {
                body.stdout.push('\n');
            }
        }
        entry.output = Some(serde_json::to_string(&body).unwrap_or_else(|_| chunk.to_string()));
    }

    pub fn set_web_preview(&mut self, key: &str, url: &str, preview: Option<String>) {
        let entry = self.entries.entry(key.to_string()).or_default();
        entry.url = Some(url.to_string());
        if preview.is_some() {
            entry.web_preview = preview;
        }
    }
}

pub fn rebuild_detail_store(events: &[deepseekcode_schema::event::EventEnvelope]) -> DetailStore {
    use deepseekcode_schema::event::EventKind;
    use deepseekcode_schema::todo::TodoItem;
    let mut store = DetailStore::default();
    for event in events {
        match event.kind {
            EventKind::ToolStarted => {
                if let Some(id) = event.payload["tool_call_id"].as_str() {
                    let input = event.payload.get("input").cloned().unwrap_or_default();
                    let detail =
                        serde_json::to_string_pretty(&input).unwrap_or_else(|_| input.to_string());
                    store.insert(
                        id,
                        DetailPayload {
                            input: Some(detail),
                            output: None,
                            todos: None,
                            title: event
                                .payload
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            url: None,
                            web_preview: None,
                        },
                    );
                }
            }
            EventKind::ToolCompleted => {
                if let Some(id) = event.payload["tool_call_id"].as_str() {
                    let output = event.payload.get("output").cloned().unwrap_or_default();
                    let prev = store.get(id).cloned();
                    let tool_name = event.payload["tool_name"].as_str().unwrap_or("");
                    let (url, web_preview) = web_fields_from_tool(tool_name, &output);
                    store.insert(
                        id,
                        DetailPayload {
                            input: prev.as_ref().and_then(|d| d.input.clone()),
                            output: Some(
                                serde_json::to_string_pretty(&output)
                                    .unwrap_or_else(|_| output.to_string()),
                            ),
                            todos: None,
                            title: event
                                .payload
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                                .or_else(|| prev.and_then(|d| d.title)),
                            url,
                            web_preview,
                        },
                    );
                }
            }
            EventKind::ToolProgress => {
                if let (Some(id), Some(chunk)) = (
                    event.payload["tool_call_id"].as_str(),
                    event.payload["chunk"].as_str(),
                ) {
                    let stream = event.payload["stream"].as_str().unwrap_or("stdout");
                    store.append_output(id, chunk, stream);
                }
            }
            EventKind::TodoUpdated => {
                let todos: Vec<TodoItem> = event
                    .payload
                    .get("todos")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                store.insert(
                    format!("todo-{}", event.seq),
                    DetailPayload {
                        input: None,
                        output: None,
                        todos: Some(todos),
                        title: Some("Updated todos".into()),
                        url: None,
                        web_preview: None,
                    },
                );
            }
            _ => {}
        }
    }
    store
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct BashStreamBody {
    stdout: String,
    stderr: String,
}

fn parse_or_init_bash_stream(existing: Option<&str>) -> BashStreamBody {
    if let Some(raw) = existing {
        if let Ok(body) = serde_json::from_str::<BashStreamBody>(raw) {
            return body;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if v.get("output").is_some() {
                return BashStreamBody::default();
            }
        }
    }
    BashStreamBody::default()
}

fn web_fields_from_tool(
    tool_name: &str,
    output: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    if !matches!(tool_name, "webfetch" | "websearch") {
        return (None, None);
    }
    let body = output.get("output").unwrap_or(output);
    let url = body
        .get("url")
        .or_else(|| {
            body.get("results")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("url"))
        })
        .and_then(|v| v.as_str())
        .map(String::from);
    let preview = body
        .get("content")
        .or_else(|| body.get("body"))
        .and_then(|v| v.as_str())
        .map(|s| truncate_preview(s, 4000))
        .or_else(|| {
            body.get("results").and_then(|r| r.as_array()).map(|arr| {
                arr.iter()
                    .take(5)
                    .filter_map(|item| {
                        Some(format!(
                            "{} — {}",
                            item.get("title")?.as_str()?,
                            item.get("url")?.as_str()?
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        });
    (url, preview)
}

fn truncate_preview(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…?", text.chars().take(max).collect::<String>())
    }
}

pub struct ActivityTimelineView;

impl ActivityTimelineView {
    pub fn render(
        cx: &gpui::Context<crate::workspace::WorkspaceView>,
        tree: &ActivityTree,
        expanded_groups: &std::collections::HashSet<String>,
        collapsed_groups: &std::collections::HashSet<String>,
        expanded_leaves: &std::collections::HashSet<String>,
        highlighted_todo_leaf: Option<&str>,
        selected_detail_key: Option<&str>,
        scroll_handle: &gpui::ScrollHandle,
        session_loading: bool,
        // True while LLM work is running. The latest task's execution node
        // stays expanded during work and collapses once the task is done.
        task_active: bool,
        text_focus: &gpui::FocusHandle,
    ) -> impl gpui::IntoElement {
        use crate::selectable_text::TIMELINE_TEXT_CONTEXT;
        use crate::ui::scrollbar::render_flex_scroll_viewport;
        use gpui::*;

        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let flattened =
            flatten_activity_tree(tree, expanded_groups, collapsed_groups, expanded_leaves);
        // IMPORTANT: do not window/virtualize rows against estimated heights.
        // Estimated spacer heights diverge from real markdown layout; wheel scrolling then
        // remounts a wrong window (blank viewport) and max_offset jumps so you cannot reach the end.
        if flattened.len() > 80 {
            tracing::debug!(
                target: deepseekcode_telemetry::DESKTOP,
                roots = tree.roots.len(),
                rows = flattened.len(),
                scroll_y = ?scroll_handle.offset().y,
                max_scroll = ?scroll_handle.max_offset().y,
                "activity timeline full render (no virtualization)"
            );
        }

        // Text-selection surface: drag-select anywhere in the timeline,
        // `Ctrl+C` copies (scoped binding), shared focus for the key context.
        let mut content = div()
            .id("activity-timeline-content")
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .key_context(TIMELINE_TEXT_CONTEXT)
            .track_focus(text_focus)
            .on_action(cx.listener(|this, _: &CopyTimelineSelection, _, cx| {
                this.copy_timeline_selection(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.text_selection_down(event.position, event.click_count, window, cx);
                }),
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

        if tree.roots.is_empty() {
            // NOTE: the spinner shows ONLY while the session is actually loading.
            // A loaded-but-empty tree (e.g. a new session whose only event is
            // `session.created`, which projects to no rows) must show the empty
            // hint — never a spinner that can never resolve.
            if session_loading {
                content = content.child(
                    div()
                        .flex_1()
                        .min_h(px(200.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_3()
                                .px_6()
                                .py_5()
                                .rounded_lg()
                                .bg(theme.bg_elevated)
                                .border_1()
                                .border_color(theme.border_faint)
                                .child(crate::ui::loader::render_loader(&theme, px(28.)))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.text_secondary)
                                        .child("正在加载会话…"),
                                ),
                        ),
                );
            } else {
                content = content.child(
                    div()
                        .flex_1()
                        .min_h(px(120.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_xs()
                                .line_height(crate::theme::leading_sm(&theme))
                                .text_color(theme.text_muted)
                                .child("发送一条消息开始对话。"),
                        ),
                );
            }
        } else if flattened.is_empty() {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("No visible activity rows."),
            );
        } else {
            // Task hierarchy: rows chunk at depth-0 user messages. Each chunk
            // renders as [user?] + execution node (turns, collapsible) +
            // trailing markers + summary sibling (hoisted final text).
            for chunk in chunk_task_rows(&flattened, task_active) {
                for element in render_task_chunk(
                    cx,
                    &chunk,
                    expanded_groups,
                    collapsed_groups,
                    expanded_leaves,
                    highlighted_todo_leaf,
                    selected_detail_key,
                    &theme,
                ) {
                    content = content.child(element);
                }
            }
        }

        div()
            .id("activity-panel")
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.bg_base)
            .child(render_flex_scroll_viewport(
                cx,
                scroll_handle,
                "activity-timeline-scroll",
                "activity-scrollbar-track",
                true,
                content,
            ))
    }
}

/// Parse `<prefix> for <N>s` / `<prefix> for <1s` labels into seconds.
fn parse_worked_secs(label: &str) -> Option<u64> {
    let rest = label.split_once(" for ")?.1;
    if rest == "<1s" {
        return Some(0);
    }
    rest.strip_suffix('s')?.parse().ok()
}

fn format_duration_secs(secs: u64) -> String {
    if secs == 0 {
        "<1s".into()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let (m, r) = (secs / 60, secs % 60);
        if r == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {r}s")
        }
    } else {
        format!("{}h {}m", secs / 3600, secs % 3600 / 60)
    }
}

/// Humanize second-precision duration labels: `Worked for 7502s` reads broken
/// (unclosed turns accumulate against `now`), so render `Worked for 2h 5m`.
/// Any label that does not match `<prefix> for <N>s` passes through unchanged.
fn humanize_worked_label(label: &str) -> String {
    match label.split_once(" for ") {
        Some((prefix, _)) => match parse_worked_secs(label) {
            Some(secs) => format!("{prefix} for {}", format_duration_secs(secs)),
            None => label.to_string(),
        },
        None => label.to_string(),
    }
}

/// One user prompt's worth of timeline: an optional user row, the execution
/// node rows (turn sections + markers, summary leaf excluded), and the
/// hoisted summary leaf that renders as the execution node's sibling.
struct TaskChunk<'a> {
    user_row: Option<&'a FlattenedActivityRow>,
    exec_rows: Vec<&'a FlattenedActivityRow>,
    summary_row: Option<&'a FlattenedActivityRow>,
    exec_id: String,
    exec_label: String,
    exec_collapsed_default: bool,
}

/// Split flattened rows into task chunks at depth-0 user messages.
fn chunk_task_rows(rows: &[FlattenedActivityRow], task_active: bool) -> Vec<TaskChunk<'_>> {
    use deepseekcode_schema::activity::{ActivityGroupKind, ActivityKind};

    // 1. Split: a depth-0 user message starts a new chunk.
    let mut raw: Vec<Vec<&FlattenedActivityRow>> = vec![Vec::new()];
    for row in rows {
        let starts_task = matches!(
            row,
            FlattenedActivityRow::Leaf { leaf, depth: 0, .. }
            if leaf.kind == ActivityKind::UserMessage
        ) && !raw.last().map(|chunk| chunk.is_empty()).unwrap_or(false);
        if starts_task {
            raw.push(Vec::new());
        }
        raw.last_mut().expect("task chunk").push(row);
    }

    // 2. Per chunk: hoist the last depth-1 assistant text as the summary
    // sibling, total turn durations for the execution label.
    let last_chunk = raw.len().saturating_sub(1);
    raw.into_iter()
        .enumerate()
        .map(|(index, chunk_rows)| {
            let user_row = chunk_rows.first().copied().filter(|row| {
                matches!(
                    row,
                    FlattenedActivityRow::Leaf { leaf, depth: 0, .. }
                    if leaf.kind == ActivityKind::UserMessage
                )
            });
            let summary_row = chunk_rows
                .iter()
                .rev()
                .find(|row| {
                    matches!(
                        row,
                        FlattenedActivityRow::Leaf { leaf, depth: 1, .. }
                        if leaf.kind == ActivityKind::AssistantMessage
                            && leaf.text.is_some()
                    )
                })
                .copied();
            let summary_id = summary_row.map(|row| match row {
                FlattenedActivityRow::Leaf { leaf, .. } => leaf.id.as_str(),
                _ => "",
            });
            let start = usize::from(user_row.is_some());
            let mut exec_rows = Vec::with_capacity(chunk_rows.len().saturating_sub(start));
            for row in chunk_rows.into_iter().skip(start) {
                let is_summary = match row {
                    FlattenedActivityRow::Leaf { leaf, .. } => Some(leaf.id.as_str()) == summary_id,
                    _ => false,
                };
                if !is_summary {
                    exec_rows.push(row);
                }
            }
            let total_secs: u64 = exec_rows
                .iter()
                .filter_map(|row| match row {
                    FlattenedActivityRow::GroupHeader { label, kind, .. }
                        if *kind == ActivityGroupKind::ProviderTurn =>
                    {
                        parse_worked_secs(label)
                    }
                    _ => None,
                })
                .sum();
            TaskChunk {
                user_row,
                exec_rows,
                summary_row,
                exec_id: format!("task-exec-{index}"),
                exec_label: format!(
                    "Executed tasks, Worked for {}",
                    format_duration_secs(total_secs)
                ),
                exec_collapsed_default: index != last_chunk || !task_active,
            }
        })
        .collect()
}

/// Render one task chunk: `[user?] + execution node + markers + summary`.
fn render_task_chunk(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    chunk: &TaskChunk<'_>,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    selected_detail_key: Option<&str>,
    theme: &crate::theme::ThemeTokens,
) -> Vec<gpui::AnyElement> {
    use crate::activity_rows::FlattenedActivityRow as Row;
    use crate::logic::is_activity_group_expanded;
    use deepseekcode_schema::activity::{ActivityGroup, ActivityGroupKind};

    let mut elements = Vec::new();
    if let Some(user) = chunk.user_row {
        elements.push(render_flattened_row(
            cx,
            user,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            theme,
        ));
    }
    // Execution node owns every turn; plain markers stay outside it so
    // failure/interrupt markers remain visible when execution is collapsed.
    let (mut turn_rows, mut marker_rows) = (Vec::new(), Vec::new());
    let mut index = 0;
    while index < chunk.exec_rows.len() {
        let row = chunk.exec_rows[index];
        let is_turn = matches!(
            row,
            Row::GroupHeader {
                depth: 0,
                kind: ActivityGroupKind::ProviderTurn,
                ..
            }
        );
        if is_turn {
            let mut end = index + 1;
            while end < chunk.exec_rows.len() && chunk.exec_rows[end].depth() > 0 {
                end += 1;
            }
            turn_rows.push(&chunk.exec_rows[index..end]);
            index = end;
        } else {
            marker_rows.push(row);
            index += 1;
        }
    }
    if !turn_rows.is_empty() {
        let dummy = ActivityGroup {
            id: chunk.exec_id.clone(),
            label: chunk.exec_label.clone(),
            kind: ActivityGroupKind::ProviderTurn,
            children: Vec::new(),
            summary: None,
            collapsed_default: chunk.exec_collapsed_default,
        };
        elements.push(render_group(
            cx,
            &dummy,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            0,
            theme,
        ));
        if is_activity_group_expanded(&dummy, expanded_groups, collapsed_groups) {
            // Second rail level: turn cards sit visibly inside their execution
            // node (indented + continuous rail), so parent/child reads at a
            // glance. Turn rows keep their own depth — the container provides
            // the offset, mirroring `render_turn_section`.
            use gpui::*;
            let mut exec_body = div()
                .flex()
                .flex_col()
                .gap_1()
                .ml(px(14.))
                .pl(px(8.))
                .border_l_1()
                .border_color(theme.border_faint);
            for run in turn_rows {
                exec_body = exec_body.child(render_turn_section(
                    cx,
                    run,
                    expanded_groups,
                    collapsed_groups,
                    expanded_leaves,
                    highlighted_todo_leaf,
                    selected_detail_key,
                    theme,
                ));
            }
            elements.push(exec_body.into_any_element());
        }
    }
    for row in marker_rows {
        elements.push(render_flattened_row(
            cx,
            row,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            theme,
        ));
    }
    if let Some(summary) = chunk.summary_row {
        elements.push(render_summary_node(cx, summary, theme));
    }
    elements
}

/// The task's final result: sibling of the execution node, always expanded.
/// Same icon/text geometry as inline thinking rows, inside an emphasis card.
fn render_summary_node(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    row: &FlattenedActivityRow,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use gpui::*;
    let FlattenedActivityRow::Leaf { leaf, .. } = row else {
        return div().into_any_element();
    };
    let Some(text) = leaf.text.clone() else {
        return div().into_any_element();
    };
    let copy_id = ElementId::Name(SharedString::from(format!("copy-summary-{}", leaf.id)));
    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .px_2()
                .text_xs()
                .line_height(crate::theme::leading_xs(theme))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child("Summary"),
        )
        .child(
            div()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded_md()
                .bg(theme.bg_elevated)
                .flex()
                .items_start()
                .gap_1p5()
                .child(crate::ui::icon::render_icon_rgba(
                    crate::ui::icon::IconName::Bot,
                    px(14.),
                    theme.info,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_xs()
                        .line_height(crate::theme::leading_relaxed(theme))
                        .text_color(theme.text_primary)
                        .child(crate::markdown::render_markdown(
                            &text,
                            theme,
                            &leaf.id,
                            &cx.entity(),
                        )),
                )
                .child(crate::ui::copy_button::render_copy_button(
                    copy_id, text, theme,
                )),
        )
        .into_any_element()
}

/// A turn section: header card + indented rail body holding the turn's rows.
/// `rows[0]` is the turn's `GroupHeader`; the rest are its descendants with
/// `depth` shifted by one so indentation stays relative to the rail instead
/// of doubling up against the full-width origin.
fn render_turn_section(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    rows: &[&FlattenedActivityRow],
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    selected_detail_key: Option<&str>,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use gpui::*;
    let mut body = div()
        .flex()
        .flex_col()
        .gap_1()
        .ml(px(14.))
        .pl(px(8.))
        .border_l_1()
        .border_color(theme.border_faint);
    for row in rows[1..].iter().copied() {
        let shifted = row.shifted(1);
        body = body.child(render_flattened_row(
            cx,
            &shifted,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            theme,
        ));
    }
    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(render_flattened_row(
            cx,
            rows[0],
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            theme,
        ))
        .child(body)
        .into_any_element()
}

/// Selectable single-style label for rows built outside markdown
/// (tool rows, headers, summaries, user text).
#[allow(clippy::too_many_arguments)]
fn selectable_label(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    block: String,
    text: String,
    color: gpui::Rgba,
    leading: gpui::Pixels,
    weight: gpui::FontWeight,
    theme: &crate::theme::ThemeTokens,
) -> impl gpui::IntoElement {
    use crate::selectable_text::selectable_plain;
    use gpui::*;
    let view = cx.entity();
    let mut ui_font = font(crate::theme::font_ui(theme));
    ui_font.weight = weight;
    let element = selectable_plain(
        &view,
        block,
        text,
        ui_font,
        color,
        theme.typography.text_xs,
        leading,
        true,
    );
    div().cursor(CursorStyle::IBeam).child(element)
}

fn render_flattened_row(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    row: &FlattenedActivityRow,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    selected_detail_key: Option<&str>,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    match row {
        FlattenedActivityRow::GroupHeader {
            group_id,
            label,
            depth,
            collapsed_default,
            kind,
            summary,
        } => {
            // Turn headers merge operation summary + duration:
            // `Explore 5 files, Worked for 12s`. Nested/sub groups keep
            // their backend label unchanged.
            let display_label = match (kind, summary) {
                (ActivityGroupKind::ProviderTurn, Some(s)) => {
                    match crate::activity_leaf::format_summary_header(s) {
                        Some(header) => format!("{header}, {}", humanize_worked_label(label)),
                        None => humanize_worked_label(label),
                    }
                }
                _ => humanize_worked_label(label),
            };
            let group = ActivityGroup {
                id: group_id.clone(),
                label: display_label,
                kind: *kind,
                children: Vec::new(),
                summary: None,
                collapsed_default: *collapsed_default,
            };
            render_group(
                cx,
                &group,
                expanded_groups,
                collapsed_groups,
                expanded_leaves,
                highlighted_todo_leaf,
                selected_detail_key,
                *depth,
                theme,
            )
        }
        FlattenedActivityRow::GroupSummary { summary, depth } => {
            use crate::activity_leaf::format_summary_line;
            use gpui::*;
            // Same indent as sibling rows: a summary belongs to its header,
            // not one level deeper than the rows it summarizes.
            div()
                .px_2()
                .pl(px((8 + depth * 12) as f32))
                .text_xs()
                .line_height(crate::theme::leading_xs(&theme))
                .text_color(theme.text_muted)
                .child(format_summary_line(summary))
                .into_any_element()
        }
        FlattenedActivityRow::Leaf { leaf, depth, .. } => render_node(
            cx,
            &ActivityNode::Leaf(leaf.clone()),
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            *depth,
            theme,
        ),
    }
}

fn render_node(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    node: &ActivityNode,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    selected_detail_key: Option<&str>,
    depth: usize,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use crate::activity_leaf::format_leaf_line;
    use deepseekcode_schema::activity::{ActivityNode, ActivityTargetKind};
    use gpui::prelude::FluentBuilder;
    use gpui::*;

    match node {
        ActivityNode::Group(group) => render_group(
            cx,
            group,
            expanded_groups,
            collapsed_groups,
            expanded_leaves,
            highlighted_todo_leaf,
            selected_detail_key,
            depth,
            theme,
        ),
        ActivityNode::Leaf(leaf) => {
            use deepseekcode_schema::activity::ActivityKind;
            let is_expanded = expanded_leaves.contains(&leaf.id)
                && leaf.text.is_some()
                && matches!(
                    leaf.kind,
                    ActivityKind::Reasoning | ActivityKind::AssistantMessage
                );
            let line = if is_expanded && leaf.kind == ActivityKind::Reasoning {
                leaf.text.clone().unwrap_or_else(|| format_leaf_line(leaf))
            } else {
                format_leaf_line(leaf)
            };
            let detail_ref = leaf.detail_ref.clone();
            let target = leaf.target.clone();
            let kind = leaf.kind;
            let meta = leaf.meta.clone();
            let leaf_id = leaf.id.clone();
            let assistant_text = leaf.text.clone();
            let is_todo_highlighted = highlighted_todo_leaf == Some(leaf_id.as_str())
                && matches!(kind, ActivityKind::TodoWrite | ActivityKind::TodoRead);
            // Row ↔ detail-panel anchor: the open Input/Output panel is keyed
            // by `selected_detail_key` (== this leaf's `detail_ref`), so the
            // source row lights up while its detail is shown.
            let is_detail_selected = match (selected_detail_key, leaf.detail_ref.as_deref()) {
                (Some(selected), Some(detail)) => selected == detail,
                _ => false,
            };
            let is_session_meta = leaf.kind == ActivityKind::SessionMeta;
            let is_reasoning = leaf.kind == ActivityKind::Reasoning;
            let is_assistant = leaf.kind == ActivityKind::AssistantMessage;
            let is_user = leaf.kind == ActivityKind::UserMessage;
            let _is_message = is_user || is_assistant;
            let theme = theme.clone();
            // Assistant thinking rows share tool-row geometry (same padding)
            // so bot icons line up with tool icons; only user/session-meta
            // rows keep the roomier message padding.
            let compact_row = !is_user && !is_session_meta;
            let mut row = div()
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(if compact_row { px(2.) } else { px(4.) })
                .px(if compact_row { px(8.) } else { px(12.) })
                .py(if compact_row { px(2.) } else { px(6.) })
                .pl(px((8 + depth * 12) as f32))
                .rounded_md()
                .when(is_todo_highlighted || is_detail_selected, |s| {
                    s.bg(theme.bg_selected)
                });

            if is_user {
                row = row.child(crate::ui::activity::render_turn_divider(&theme));
                if let Some(text) = &leaf.text {
                    row = row.child(crate::ui::activity::render_user_message_card(
                        &leaf_id,
                        &text,
                        &theme,
                        &cx.entity(),
                    ));
                } else {
                    row = row.child(
                        div()
                            .text_xs()
                            .line_height(crate::theme::leading_base(&theme))
                            .text_color(theme.text_primary)
                            .child(line),
                    );
                }
            } else if is_assistant {
                if let Some(text) = assistant_text {
                    // Same geometry as tool rows: bot icon in the icon column,
                    // markdown content in the text column, copy trailing.
                    let copy_id =
                        ElementId::Name(SharedString::from(format!("copy-assistant-{leaf_id}")));
                    row = row.child(
                        div()
                            .flex()
                            .items_start()
                            .gap_1p5()
                            .child(crate::ui::icon::render_icon_rgba(
                                crate::ui::icon::IconName::Bot,
                                px(14.),
                                theme.info,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_xs()
                                    .line_height(crate::theme::leading_relaxed(&theme))
                                    .text_color(theme.text_primary)
                                    .child(crate::markdown::render_markdown(
                                        &text,
                                        &theme,
                                        &leaf_id,
                                        &cx.entity(),
                                    )),
                            )
                            .child(crate::ui::copy_button::render_copy_button(
                                copy_id, text, &theme,
                            )),
                    );
                } else {
                    row = row.child(
                        div()
                            .text_xs()
                            .line_height(crate::theme::leading_base(&theme))
                            .text_color(theme.text_primary)
                            .child(line),
                    );
                }
            } else if is_session_meta {
                row = row.child(
                    div()
                        .w_full()
                        .text_center()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(line),
                );
            } else {
                use deepseekcode_schema::activity::ActivityStatus;
                let (icon, color) = activity_kind_style(kind, leaf.status, &theme);
                let is_running = leaf.status == ActivityStatus::Running;
                row = row.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_1p5()
                        .child(if is_running {
                            crate::ui::loader::render_loader(&theme, px(14.)).into_any_element()
                        } else {
                            crate::ui::icon::render_icon_rgba(icon, px(14.), color)
                                .into_any_element()
                        })
                        .child(div().flex_1().min_w(px(0.)).child(selectable_label(
                            cx,
                            format!("{leaf_id}#line"),
                            line.clone(),
                            color,
                            crate::theme::leading_sm(&theme),
                            gpui::FontWeight::NORMAL,
                            &theme,
                        ))),
                );
            }
            // Deferred taps: record the press on mouse-down, act on mouse-up
            // only when the pointer did not drag — text selection wins over
            // row clicks (see `selectable_text::TapCallback`).
            let tap: Option<TapCallback> = if is_reasoning && leaf.text.is_some() {
                let lid = leaf_id.clone();
                Some(Rc::new(
                    move |this: &mut crate::workspace::WorkspaceView,
                          _: &mut gpui::Window,
                          cx: &mut gpui::Context<crate::workspace::WorkspaceView>| {
                        this.toggle_activity_leaf(&lid, cx);
                    },
                ))
            } else if is_assistant && leaf.text.is_some() {
                let lid = leaf_id.clone();
                Some(Rc::new(
                    move |this: &mut crate::workspace::WorkspaceView,
                          _: &mut gpui::Window,
                          cx: &mut gpui::Context<crate::workspace::WorkspaceView>| {
                        this.toggle_activity_leaf(&lid, cx);
                        if let Ok(message_id) = deepseekcode_types::MessageId::from_str(&lid) {
                            this.select_timeline_message(message_id, cx);
                        }
                    },
                ))
            } else if is_user {
                let lid = leaf_id.clone();
                Some(Rc::new(
                    move |this: &mut crate::workspace::WorkspaceView,
                          _: &mut gpui::Window,
                          cx: &mut gpui::Context<crate::workspace::WorkspaceView>| {
                        if let Ok(message_id) = deepseekcode_types::MessageId::from_str(&lid) {
                            this.select_timeline_message(message_id, cx);
                        }
                    },
                ))
            } else if detail_ref.is_some() || !matches!(target, ActivityTargetKind::None) {
                let tap_id = leaf_id.clone();
                Some(Rc::new(
                    move |this: &mut crate::workspace::WorkspaceView,
                          _: &mut gpui::Window,
                          cx: &mut gpui::Context<crate::workspace::WorkspaceView>| {
                        this.route_activity_leaf_click(
                            &tap_id,
                            kind,
                            &target,
                            &meta,
                            detail_ref.as_deref(),
                            cx,
                        );
                    },
                ))
            } else {
                None
            };
            if tap.is_some() {
                let press_id = leaf_id.clone();
                let tap_id = leaf_id.clone();
                row = row
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.text_row_press(&press_id, event.position);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.text_row_tap(&tap_id, event.position, tap.clone(), window, cx);
                        }),
                    );
            }
            row.into_any_element()
        }
    }
}

fn render_group(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    group: &ActivityGroup,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    selected_detail_key: Option<&str>,
    depth: usize,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use crate::activity_leaf::format_summary_line;
    use crate::logic::is_activity_group_expanded;
    use gpui::prelude::FluentBuilder;
    use gpui::*;

    let is_expanded = is_activity_group_expanded(group, expanded_groups, collapsed_groups);
    let gid = group.id.clone();
    let collapsed_default = group.collapsed_default;
    let is_session_meta = group.kind == ActivityGroupKind::SessionMeta;
    // Nested Thought / TaskSubagent groups render weaker than turn headers so
    // the hierarchy reads: turn card > sub-group row > tool row.
    let is_sub_group = !matches!(
        group.kind,
        ActivityGroupKind::ProviderTurn | ActivityGroupKind::SessionMeta
    );
    let mut block = div().w_full().flex_shrink_0().flex().flex_col().gap_1();

    let theme = theme.clone();
    let chevron = if is_expanded {
        crate::ui::icon::IconName::ChevronDown
    } else {
        crate::ui::icon::IconName::ChevronRight
    };

    let tap: TapCallback = {
        let tap_id = gid.clone();
        Rc::new(
            move |this: &mut crate::workspace::WorkspaceView,
                  _: &mut gpui::Window,
                  cx: &mut gpui::Context<crate::workspace::WorkspaceView>| {
                this.toggle_activity_group(&tap_id, collapsed_default, cx);
            },
        )
    };
    let press_id = gid.clone();
    let tap_id = gid.clone();
    let header = div()
        .px_2()
        .py_1p5()
        .pl(px((8 + depth * 12) as f32))
        .rounded_md()
        .cursor_pointer()
        .when(!is_sub_group, |s| s.bg(theme.bg_elevated))
        .when(is_sub_group, |s| s.hover(|h| h.bg(theme.bg_hover)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.text_row_press(&press_id, event.position);
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                this.text_row_tap(&tap_id, event.position, Some(tap.clone()), window, cx);
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(crate::ui::icon::render_icon_rgba(
                    chevron,
                    px(if is_sub_group { 12. } else { 14. }),
                    theme.text_muted,
                ))
                .child(selectable_label(
                    cx,
                    format!("{gid}#label"),
                    group.label.clone(),
                    if is_sub_group || is_session_meta {
                        theme.text_muted
                    } else {
                        theme.text_primary
                    },
                    crate::theme::leading_sm(&theme),
                    gpui::FontWeight::MEDIUM,
                    &theme,
                )),
        );

    block = block.child(header);

    if let Some(summary) = &group.summary {
        if is_expanded {
            block = block.child(
                div()
                    .px_2()
                    .pl(px((8 + depth * 12) as f32))
                    .text_xs()
                    .line_height(crate::theme::leading_xs(&theme))
                    .text_color(theme.text_muted)
                    .child(format_summary_line(summary)),
            );
        }
    }

    if is_expanded {
        for child in &group.children {
            block = block.child(render_node(
                cx,
                child,
                expanded_groups,
                collapsed_groups,
                expanded_leaves,
                highlighted_todo_leaf,
                selected_detail_key,
                depth + 1,
                &theme,
            ));
        }
    }

    block.into_any_element()
}

fn activity_kind_style(
    kind: deepseekcode_schema::activity::ActivityKind,
    status: deepseekcode_schema::activity::ActivityStatus,
    theme: &crate::theme::ThemeTokens,
) -> (crate::ui::icon::IconName, gpui::Rgba) {
    use crate::ui::icon::IconName;
    use deepseekcode_schema::activity::{ActivityKind, ActivityStatus};

    if status == ActivityStatus::Failed {
        return (IconName::AlertCircle, theme.error);
    }
    if status == ActivityStatus::Running {
        return (IconName::Loader, theme.warning);
    }
    if status == ActivityStatus::Stale {
        return (IconName::Clock, theme.warning);
    }

    let icon = match kind {
        ActivityKind::Read | ActivityKind::Glob | ActivityKind::List => IconName::BookOpen,
        ActivityKind::Write | ActivityKind::Edit | ActivityKind::ApplyPatch => IconName::Pencil,
        ActivityKind::Grep | ActivityKind::WebSearch => IconName::Search,
        ActivityKind::Bash => IconName::Terminal,
        ActivityKind::WebFetch => IconName::ExternalLink,
        ActivityKind::TodoWrite | ActivityKind::TodoRead => IconName::List,
        ActivityKind::Reasoning => IconName::Sparkles,
        ActivityKind::Error => IconName::AlertCircle,
        _ => IconName::Zap,
    };

    let color = match kind {
        ActivityKind::Read
        | ActivityKind::Glob
        | ActivityKind::List
        | ActivityKind::Grep
        | ActivityKind::WebSearch => theme.tool_read,
        ActivityKind::Write | ActivityKind::Edit | ActivityKind::ApplyPatch => theme.tool_write,
        ActivityKind::Bash => theme.tool_bash,
        ActivityKind::Error => theme.error,
        _ => theme.text_primary,
    };

    (icon, color)
}

#[cfg(test)]
mod tests {
    use super::humanize_worked_label;
    use crate::activity_rows::FlattenedActivityRow;

    #[test]
    fn humanize_keeps_short_durations() {
        assert_eq!(humanize_worked_label("Worked for <1s"), "Worked for <1s");
        assert_eq!(humanize_worked_label("Worked for 5s"), "Worked for 5s");
        assert_eq!(humanize_worked_label("Worked for 59s"), "Worked for 59s");
    }

    #[test]
    fn humanize_folds_minutes_and_hours() {
        assert_eq!(humanize_worked_label("Worked for 60s"), "Worked for 1m");
        assert_eq!(humanize_worked_label("Worked for 62s"), "Worked for 1m 2s");
        assert_eq!(
            humanize_worked_label("Worked for 7502s"),
            "Worked for 2h 5m"
        );
    }

    #[test]
    fn humanize_passes_through_unknown_labels() {
        assert_eq!(
            humanize_worked_label("Session interrupted"),
            "Session interrupted"
        );
        assert_eq!(humanize_worked_label("Thought for 3s"), "Thought for 3s");
    }

    fn header_row(id: &str, label: &str) -> FlattenedActivityRow {
        FlattenedActivityRow::GroupHeader {
            group_id: id.into(),
            label: label.into(),
            depth: 0,
            collapsed_default: false,
            kind: deepseekcode_schema::activity::ActivityGroupKind::ProviderTurn,
            summary: None,
        }
    }

    fn assistant_row(id: &str) -> FlattenedActivityRow {
        FlattenedActivityRow::Leaf {
            leaf: deepseekcode_schema::activity::ActivityLeaf {
                id: id.into(),
                kind: deepseekcode_schema::activity::ActivityKind::AssistantMessage,
                verb: "Assistant".into(),
                target: deepseekcode_schema::activity::ActivityTargetKind::None,
                meta: Default::default(),
                status: deepseekcode_schema::activity::ActivityStatus::Completed,
                detail_ref: None,
                text: Some("final words".into()),
                started_at: None,
            },
            depth: 1,
            message_expanded: false,
        }
    }

    fn user_row(id: &str) -> FlattenedActivityRow {
        FlattenedActivityRow::Leaf {
            leaf: deepseekcode_schema::activity::ActivityLeaf {
                id: id.into(),
                kind: deepseekcode_schema::activity::ActivityKind::UserMessage,
                verb: "You".into(),
                target: deepseekcode_schema::activity::ActivityTargetKind::None,
                meta: Default::default(),
                status: deepseekcode_schema::activity::ActivityStatus::Completed,
                detail_ref: None,
                text: Some("go".into()),
                started_at: None,
            },
            depth: 0,
            message_expanded: false,
        }
    }

    #[test]
    fn chunks_split_at_user_messages_and_hoist_last_assistant() {
        let rows = vec![
            header_row("turn-1", "Worked for 60s"),
            assistant_row("a1"),
            user_row("u1"),
            header_row("turn-2", "Worked for 30s"),
        ];
        let chunks = super::chunk_task_rows(&rows, false);
        assert_eq!(chunks.len(), 2);
        // First chunk: header + hoisted summary, no user row.
        assert!(chunks[0].user_row.is_none());
        assert!(chunks[0].summary_row.is_some());
        assert_eq!(chunks[0].exec_rows.len(), 1);
        assert_eq!(chunks[0].exec_label, "Executed tasks, Worked for 1m");
        // Second chunk owns the user row; its turn has no assistant.
        assert!(chunks[1].user_row.is_some());
        assert!(chunks[1].summary_row.is_none());
        assert_eq!(chunks[1].exec_label, "Executed tasks, Worked for 30s");
    }

    #[test]
    fn execution_collapses_when_idle_and_expands_while_active() {
        let rows = vec![
            user_row("u1"),
            header_row("turn-1", "Worked for 5s"),
            user_row("u2"),
            header_row("turn-2", "Worked for 5s"),
        ];
        let idle = super::chunk_task_rows(&rows, false);
        assert!(idle.iter().all(|c| c.exec_collapsed_default));
        let active = super::chunk_task_rows(&rows, true);
        assert!(active[0].exec_collapsed_default);
        assert!(!active[1].exec_collapsed_default);
        assert_eq!(active[0].exec_id, "task-exec-0");
        assert_eq!(active[1].exec_id, "task-exec-1");
    }
}
