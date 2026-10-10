use crate::ui::button::icon_button_shell;
use crate::ui::icon::IconName;
use gpui::*;
use std::sync::{Arc, LazyLock};

static APP_ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../assets/icon.png").to_vec(),
    ))
});

pub struct TitlebarView;

impl TitlebarView {
    pub fn render(
        window: &Window,
        cx: &Context<crate::workspace::WorkspaceView>,
        health_ok: bool,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let title = deepseekcode_i18n::t("app.name");

        div()
            .id("titlebar")
            .h(px(36.))
            .w_full()
            .flex()
            .items_center()
            .bg(theme.bg_surface)
            .border_b_1()
            .border_color(theme.border_default)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        img(Arc::clone(&APP_ICON))
                            .w(px(18.))
                            .h(px(18.))
                            .rounded(px(4.)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .child(
                                div()
                                    .id("titlebar-health")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| crate::git_panel::GitFileTooltip {
                                            text: if health_ok {
                                                deepseekcode_i18n::t("titlebar.connected").into()
                                            } else {
                                                deepseekcode_i18n::t("settings.health_desc").into()
                                            },
                                        })
                                        .into()
                                    })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            // P2 T4: health is actionable — click
                                            // retries the health poll instead of
                                            // being a dead indicator.
                                            cx.stop_propagation();
                                            this.refresh_health(cx);
                                        }),
                                    )
                                    .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(
                                        if health_ok {
                                            theme.success
                                        } else {
                                            theme.error
                                        },
                                    ))
                                    .child(div().text_xs().text_color(theme.text_muted).child(
                                        if health_ok {
                                            deepseekcode_i18n::t("titlebar.connected")
                                        } else {
                                            deepseekcode_i18n::t("titlebar.offline")
                                        },
                                    )),
                            )
                            .child(
                                icon_button_shell(
                                    "titlebar-settings",
                                    IconName::Settings,
                                    &theme,
                                    px(28.),
                                    false,
                                    false,
                                    false,
                                )
                                .tooltip(move |_, cx| {
                                    cx.new(|_| crate::git_panel::GitFileTooltip {
                                        text: deepseekcode_i18n::t("titlebar.settings").into(),
                                    })
                                    .into()
                                })
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_settings(
                                            crate::settings::SettingsSection::General,
                                            cx,
                                        );
                                    }),
                                ),
                            ),
                    )
                    .child(render_window_controls(window, &theme)),
            )
    }
}

fn render_window_controls(window: &Window, theme: &crate::theme::ThemeTokens) -> impl IntoElement {
    #[cfg(target_os = "windows")]
    {
        render_windows_controls(window, theme)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, theme);
        div()
    }
}

#[cfg(target_os = "windows")]
fn render_windows_controls(window: &Window, theme: &crate::theme::ThemeTokens) -> impl IntoElement {
    div()
        .id("windows-window-controls")
        .font_family("Segoe Fluent Icons")
        .flex()
        .flex_row()
        .h(px(36.))
        .child(caption_button(
            "minimize",
            "\u{e921}",
            WindowControlArea::Min,
            false,
            theme,
        ))
        .child(if window.is_maximized() {
            caption_button("restore", "\u{e923}", WindowControlArea::Max, false, theme)
        } else {
            caption_button("maximize", "\u{e922}", WindowControlArea::Max, false, theme)
        })
        .child(caption_button(
            "close",
            "\u{e8bb}",
            WindowControlArea::Close,
            true,
            theme,
        ))
}

#[cfg(target_os = "windows")]
fn caption_button(
    id: &'static str,
    icon: &'static str,
    area: WindowControlArea,
    is_close: bool,
    theme: &crate::theme::ThemeTokens,
) -> impl IntoElement {
    // P2: Segoe Fluent Icons may be missing (tofu blocks) — fall back to a
    // text glyph via font stack so the control stays legible.
    div()
        .id(id)
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.))
        .h_full()
        .text_size(px(10.))
        .font_family("Segoe Fluent Icons, Segoe UI Symbol, sans-serif")
        .text_color(theme.text_secondary)
        .hover(|style| {
            if is_close {
                style
                    .bg(theme.window_close_hover)
                    .text_color(theme.text_on_accent)
            } else {
                style.bg(theme.bg_hover)
            }
        })
        .window_control_area(area)
        .child(icon)
}
