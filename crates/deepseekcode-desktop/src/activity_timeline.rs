use crate::activity_rows::{flatten_activity_tree, FlattenedActivityRow};
use deepseekcode_schema::activity::{ActivityGroup, ActivityGroupKind, ActivityNode, ActivityTree};
use deepseekcode_schema::todo::TodoItem;
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
        format!("{}…", text.chars().take(max).collect::<String>())
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
        scroll_handle: &gpui::ScrollHandle,
        session_loading: bool,
        has_activity_history: bool,
    ) -> impl gpui::IntoElement {
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

        let mut content = div()
            .id("activity-timeline-content")
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2();

        if tree.roots.is_empty() {
            if session_loading || has_activity_history {
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
                                .child(div().text_sm().text_color(theme.text_secondary).child(
                                    if session_loading {
                                        "正在加载会话…"
                                    } else {
                                        "正在更新会话记录…"
                                    },
                                )),
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
            for row in &flattened {
                content = content.child(render_flattened_row(
                    cx,
                    row,
                    expanded_groups,
                    collapsed_groups,
                    expanded_leaves,
                    highlighted_todo_leaf,
                    &theme,
                ));
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

fn render_flattened_row(
    cx: &gpui::Context<crate::workspace::WorkspaceView>,
    row: &FlattenedActivityRow,
    expanded_groups: &std::collections::HashSet<String>,
    collapsed_groups: &std::collections::HashSet<String>,
    expanded_leaves: &std::collections::HashSet<String>,
    highlighted_todo_leaf: Option<&str>,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    match row {
        FlattenedActivityRow::GroupHeader {
            group_id,
            label,
            depth,
            collapsed_default,
            kind,
        } => {
            let group = ActivityGroup {
                id: group_id.clone(),
                label: label.clone(),
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
                *depth,
                theme,
            )
        }
        FlattenedActivityRow::GroupSummary { summary, depth } => {
            use crate::activity_leaf::format_summary_line;
            use gpui::*;
            div()
                .px_2()
                .pl(px((16 + depth * 12) as f32))
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
    depth: usize,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use crate::activity_leaf::format_leaf_line;
    use deepseekcode_schema::activity::{ActivityNode, ActivityStatus, ActivityTargetKind};
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
            let is_session_meta = leaf.kind == ActivityKind::SessionMeta;
            let is_reasoning = leaf.kind == ActivityKind::Reasoning;
            let is_assistant = leaf.kind == ActivityKind::AssistantMessage;
            let is_user = leaf.kind == ActivityKind::UserMessage;
            let _is_message = is_user || is_assistant;
            let theme = theme.clone();
            let compact_row = !is_user && !is_assistant && !is_session_meta;
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
                .when(is_todo_highlighted, |s| s.bg(theme.bg_selected));

            if is_user {
                row = row.child(crate::ui::activity::render_turn_divider(&theme));
                if let Some(text) = &leaf.text {
                    row = row.child(crate::ui::activity::render_user_message_card(
                        &leaf_id, &text, &theme,
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
                    row = row.child(crate::ui::activity::render_assistant_header(
                        &leaf_id, &text, &theme,
                    ));
                    row = row.child(
                        div()
                            .px_2()
                            .py_1p5()
                            .rounded_md()
                            .bg(theme.bg_elevated)
                            .border_1()
                            .border_color(theme.border_faint)
                            .text_xs()
                            .line_height(crate::theme::leading_relaxed(&theme))
                            .text_color(theme.text_primary)
                            .child(crate::markdown::render_markdown(&text, &theme)),
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
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .line_height(crate::theme::leading_sm(&theme))
                                .text_color(color)
                                .child(line),
                        ),
                );
            }
            if is_reasoning && leaf.text.is_some() {
                let lid = leaf_id.clone();
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_activity_leaf(&lid, cx);
                    })
                });
            } else if is_assistant && leaf.text.is_some() {
                let lid = leaf_id.clone();
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_activity_leaf(&lid, cx);
                        if let Ok(message_id) = deepseekcode_types::MessageId::from_str(&lid) {
                            this.select_timeline_message(message_id, cx);
                        }
                    })
                });
            } else if is_user {
                let lid = leaf_id.clone();
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        if let Ok(message_id) = deepseekcode_types::MessageId::from_str(&lid) {
                            this.select_timeline_message(message_id, cx);
                        }
                    })
                });
            } else if detail_ref.is_some() || !matches!(target, ActivityTargetKind::None) {
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.route_activity_leaf_click(
                            &leaf_id,
                            kind,
                            &target,
                            &meta,
                            detail_ref.as_deref(),
                            cx,
                        );
                    })
                });
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
    depth: usize,
    theme: &crate::theme::ThemeTokens,
) -> gpui::AnyElement {
    use crate::activity_leaf::format_summary_line;
    use crate::logic::is_activity_group_expanded;
    use gpui::*;

    let is_expanded = is_activity_group_expanded(group, expanded_groups, collapsed_groups);
    let gid = group.id.clone();
    let collapsed_default = group.collapsed_default;
    let is_session_meta = group.kind == ActivityGroupKind::SessionMeta;
    let mut block = div().w_full().flex_shrink_0().flex().flex_col().gap_1();

    let theme = theme.clone();
    let chevron = if is_expanded {
        crate::ui::icon::IconName::ChevronDown
    } else {
        crate::ui::icon::IconName::ChevronRight
    };

    let header = div()
        .px_2()
        .py_1p5()
        .pl(px((8 + depth * 12) as f32))
        .rounded_md()
        .cursor_pointer()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_subtle)
        .on_mouse_down(MouseButton::Left, {
            cx.listener(move |this, _, _, cx| {
                this.toggle_activity_group(&gid, collapsed_default, cx);
            })
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(crate::ui::icon::render_icon_rgba(
                    chevron,
                    px(14.),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_xs()
                        .line_height(crate::theme::leading_sm(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_session_meta {
                            theme.text_muted
                        } else {
                            theme.text_primary
                        })
                        .child(group.label.clone()),
                ),
        );

    block = block.child(header);

    if let Some(summary) = &group.summary {
        if is_expanded {
            block = block.child(
                div()
                    .px_2()
                    .pl(px((16 + depth * 12) as f32))
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
