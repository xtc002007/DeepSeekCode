use crate::theme::ThemeTokens;
use crate::ui::copy_button::render_copy_button;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub fn render_user_message_card(
    leaf_id: &str,
    text: &str,
    theme: &ThemeTokens,
    view: &gpui::Entity<crate::workspace::WorkspaceView>,
) -> impl IntoElement {
    use crate::selectable_text::selectable_plain;
    let theme = theme.clone();
    let copy_text = text.to_string();
    let copy_id = ElementId::Name(SharedString::from(format!("copy-user-{leaf_id}")));
    let body = selectable_plain(
        view,
        format!("{leaf_id}#user"),
        text.to_string(),
        gpui::font(crate::theme::font_ui(&theme)),
        theme.text_primary,
        theme.typography.text_xs,
        crate::theme::leading_relaxed(&theme),
        true,
    );
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pl(px(2.))
        .border_l_2()
        .border_color(theme.accent_rgba())
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(render_icon_rgba(IconName::User, px(12.), theme.text_muted))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(deepseekcode_i18n::t("activity.you")),
                        ),
                )
                .child(render_copy_button(copy_id, copy_text, &theme)),
        )
        .child({
            div()
                .px_2()
                .py_1p5()
                .rounded_md()
                .bg(theme.accent_muted)
                .cursor(CursorStyle::IBeam)
                .child(body)
        })
}
