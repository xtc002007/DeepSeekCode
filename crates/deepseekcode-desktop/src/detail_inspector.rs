use crate::activity_timeline::DetailPayload;
use crate::markdown::render_markdown;
use gpui::*;

pub struct DetailInspector;

impl DetailInspector {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        detail: Option<&DetailPayload>,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        match detail {
            None => div()
                .id("detail-inspector-empty")
                .p_3()
                .text_sm()
                .text_color(theme.text_muted)
                .child("Select an activity row to inspect details."),
            Some(d) => {
                // 详情面板允许纵向滚动：长 Output/JSON 不再被 `overflow_hidden`
                // 静默截断（曾表现为输出缺字）。
                // NOTE: GPUI 的 scroll 方法挂在 `Stateful<Div>` 上，必须先 `.id()`。
                let mut body = div()
                    .id("detail-inspector-body")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .overflow_y_scroll();
                if let Some(title) = &d.title {
                    body = body.child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_secondary)
                            .child(title.clone()),
                    );
                }
                if let Some(url) = &d.url {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_link)
                                    .child(format!("URL: {url}")),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded_md()
                                    .bg(theme.bg_selected)
                                    .text_xs()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_mouse_down(MouseButton::Left, {
                                        let url = url.clone();
                                        cx.listener(move |this, _, _, _| {
                                            this.open_external_url(&url);
                                        })
                                    })
                                    .child("Open in browser"),
                            ),
                    );
                }
                if let Some(preview) = &d.web_preview {
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(section_label("Web preview", &theme))
                            .child(
                                div()
                                    .id("detail-web-preview")
                                    .p_2()
                                    .rounded_md()
                                    .bg(theme.bg_base)
                                    .max_h(px(200.))
                                    .overflow_y_scroll()
                                    .child(render_markdown(
                                        preview,
                                        &theme,
                                        d.title.as_deref().unwrap_or("detail"),
                                        &cx.entity(),
                                    )),
                            ),
                    );
                } else if let Some(url) = &d.url {
                    body = body.child(section(
                        "Web preview",
                        &format!(
                            "Fetched content preview unavailable.\nOpen the URL in your browser to view the page.\n{url}"
                        ),
                        &theme,
                    ));
                }
                if let Some(input) = &d.input {
                    body = body.child(section("Input", input, &theme));
                }
                if let Some(output) = &d.output {
                    if d.url.is_some() || d.web_preview.is_some() {
                        body = body.child(section("Raw output", output, &theme));
                    } else {
                        body = body.child(section("Output", output, &theme));
                    }
                }
                if let Some(todos) = &d.todos {
                    let text = todos
                        .iter()
                        .map(|t| format!("[{}] {} ({})", t.status, t.content, t.priority))
                        .collect::<Vec<_>>()
                        .join("\n");
                    body = body.child(section("Todos", &text, &theme));
                }
                body
            }
        }
    }
}

fn section_label(label: &str, theme: &crate::theme::ThemeTokens) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .child(label.to_string())
}

fn section(label: &str, text: &str, theme: &crate::theme::ThemeTokens) -> impl IntoElement {
    use crate::ui::multiline_text::render_multiline;
    let mono = crate::theme::font_mono(theme);
    let fg = theme.text_primary;
    // NOTE: scroll 需要 `.id()`（GPUI `Stateful<Div>`），用 label 区分
    // Input/Output/Todos 区块（同时仅展示一个详情面板，不会碰撞）。
    let scroll_id = ElementId::Name(format!("detail-section-{label}").into());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section_label(label, theme))
        .child(
            div()
                .id(scroll_id)
                .p_2()
                .rounded_md()
                .bg(theme.bg_base)
                .max_h(px(320.))
                .overflow_y_scroll()
                .overflow_x_scroll()
                .child(render_multiline(text, move |d| {
                    d.text_xs().font_family(mono.clone()).text_color(fg)
                })),
        )
}
