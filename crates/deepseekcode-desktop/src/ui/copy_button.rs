use crate::theme::ThemeTokens;
use crate::ui::button::render_icon_button;
use crate::ui::icon::IconName;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

thread_local! {
    static LAST_COPY_AT: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
}

fn copy_feedback_key(id: &ElementId) -> String {
    format!("{id:?}")
}

fn mark_copied(id: &ElementId) {
    LAST_COPY_AT.with(|m| {
        m.borrow_mut().insert(copy_feedback_key(id), Instant::now());
    });
}

fn recently_copied(id: &ElementId) -> bool {
    LAST_COPY_AT.with(|m| {
        m.borrow()
            .get(&copy_feedback_key(id))
            .is_some_and(|t| t.elapsed().as_millis() < 1500)
    })
}

pub fn render_copy_button(
    id: impl Into<ElementId>,
    text: impl Into<String>,
    theme: &ThemeTokens,
) -> impl IntoElement {
    let element_id: ElementId = id.into();
    let text = text.into();
    let theme = theme.clone();
    if text.is_empty() {
        // P2: never wash the clipboard with an empty string — render a muted
        // non-interactive icon instead of a live copy button.
        return div()
            .id(element_id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.))
            .rounded_sm()
            .opacity(0.4)
            .tooltip(move |_, cx| {
                cx.new(|_| crate::git_panel::GitFileTooltip {
                    text: deepseekcode_i18n::t("common.nothing_to_copy").into(),
                })
                .into()
            })
            .child(crate::ui::icon::render_icon_rgba(
                IconName::Copy,
                px(16.),
                theme.text_disabled,
            ))
            .into_any_element();
    }
    // P2: success feedback — show Check for 1.5s after a copy (was: always
    // Copy, user couldn't tell if it worked). P1-10: stop so copy never also
    // selects the parent row (render_icon_button already stops).
    let show_check = recently_copied(&element_id);
    let icon = if show_check {
        IconName::Check
    } else {
        IconName::Copy
    };
    let element_id_for_cb = element_id.clone();
    render_icon_button(
        element_id,
        icon,
        &theme,
        px(24.),
        false,
        false,
        move |cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            mark_copied(&element_id_for_cb);
        },
    )
    .into_any_element()
}
