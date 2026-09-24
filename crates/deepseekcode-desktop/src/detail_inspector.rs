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
                .p_3()
                .text_sm()
                .text_color(theme.text_muted)
                .child("Select an activity row to inspect details."),
            Some(d) => {
                let mut body = div().flex().flex_col().gap_2().p_3().overflow_hidden();
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
                                    .p_2()
                                    .rounded_md()
                                    .bg(theme.bg_base)
                                    .max_h(px(200.))
                                    .overflow_hidden()
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
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section_label(label, theme))
        .child(
            div()
                .p_2()
                .rounded_md()
                .bg(theme.bg_base)
                .text_xs()
                .font_family(crate::theme::font_mono(theme))
                .text_color(theme.text_primary)
                .child(text.to_string()),
        )
}
