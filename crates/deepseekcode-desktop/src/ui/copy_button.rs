use crate::theme::ThemeTokens;
use crate::ui::button::render_icon_button;
use crate::ui::icon::IconName;
use gpui::*;

pub fn render_copy_button(
    id: impl Into<ElementId>,
    text: impl Into<String>,
    theme: &ThemeTokens,
) -> impl IntoElement {
    let text = text.into();
    let theme = theme.clone();
    render_icon_button(id, IconName::Copy, &theme, px(24.), false, false, move |cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    })
}
