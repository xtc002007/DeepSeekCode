use crate::logic::GitStatusEntry;
use gpui::*;

pub struct GitPanel;

impl GitPanel {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        summary: Option<&str>,
        status_message: Option<&str>,
        staged: &[GitStatusEntry],
        unstaged: &[GitStatusEntry],
        untracked: &[GitStatusEntry],
        log: &[String],
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let mut list = div().flex().flex_col().gap_2().p_2();
        list = list.child(
            div()
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .justify_start()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme.bg_elevated)
                        .hover(|s| s.bg(theme.bg_hover))
                        .on_mouse_down(MouseButton::Left, {
                            cx.listener(|this, _, _, cx| {
                                this.refresh_git_status_async(cx);
                            })
                        })
                        .child(deepseekcode_i18n::t("common.refresh")),
                ),
        );
        if let Some(message) = status_message.filter(|s| !s.is_empty()) {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(message.to_string()),
            );
        } else if let Some(summary) = summary.filter(|s| !s.is_empty()) {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_family(crate::theme::font_mono(&theme))
                    .text_color(theme.text_muted)
                    .child(summary.to_string()),
            );
        }
        if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
            if status_message.is_none() {
                list = list.child(
                    div()
                        .px_2()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child(deepseekcode_i18n::t("git.clean")),
                );
            }
        } else {
            list = list
                .child(render_section(
                    cx,
                    &deepseekcode_i18n::t("git.staged"),
                    staged,
                    theme.success,
                    &theme,
                ))
                .child(render_section(
                    cx,
                    &deepseekcode_i18n::t("git.unstaged"),
                    unstaged,
                    theme.warning,
                    &theme,
                ))
                .child(render_section(
                    cx,
                    &deepseekcode_i18n::t("git.untracked"),
                    untracked,
                    theme.text_muted,
                    &theme,
                ));
        }
        if !log.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_link)
                    .child(deepseekcode_i18n::t("git.recent_commits")),
            );
            for line in log {
                list = list.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .font_family(crate::theme::font_mono(&theme))
                        .text_color(theme.text_secondary)
                        .child(line.clone()),
                );
            }
        }
        div()
            .id("git-panel")
            .size_full()
            .overflow_y_scroll()
            .child(list)
    }
}

fn render_section(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    title: &str,
    entries: &[GitStatusEntry],
    accent: Rgba,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    if entries.is_empty() {
        return div().into_any_element();
    }
    let mut section = div().flex().flex_col().gap_1().child(
        div()
            .px_2()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(accent)
            .child(title.to_string()),
    );
    for entry in entries {
        let path = entry.path.clone();
        let label = if !entry.staged_label.is_empty() {
            format!(
                "{}  {}",
                entry.raw.chars().take(2).collect::<String>(),
                entry.path
            )
        } else {
            entry.raw.clone()
        };
        section = section.child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .text_xs()
                .font_family(crate::theme::font_mono(&theme))
                .text_color(theme.text_secondary)
                .on_mouse_down(MouseButton::Left, {
                    cx.listener(move |this, _, _, cx| {
                        this.open_git_changed_file(&path, cx);
                    })
                })
                .child(label),
        );
    }
    section.into_any_element()
}
