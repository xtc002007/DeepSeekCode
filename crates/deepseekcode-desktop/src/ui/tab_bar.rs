use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon, IconName};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabItem {
    pub id: &'static str,
    pub label: String,
    pub icon: IconName,
}

/// Single tab item shell — attach `.on_mouse_down(cx.listener(...))` at the call site.
pub fn tab_item_shell(tab: TabItem, active: bool, theme: &ThemeTokens) -> Stateful<Div> {
    let theme = theme.clone();
    let text_color = if active {
        theme.text_primary
    } else {
        theme.text_muted
    };
    let icon_color: Hsla = text_color.into();
    let accent_border: Hsla = theme.accent;
    let transparent_border = hsla(0., 0., 0., 0.);
    let bg_hover = theme.bg_hover;

    div()
        .id(ElementId::Name(SharedString::from(format!(
            "tab-{}",
            tab.id
        ))))
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(10.))
        .py(px(6.))
        .cursor_pointer()
        .text_xs()
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(text_color)
        .border_b_2()
        .when(active, |s| s.border_color(accent_border))
        .when(!active, |s| s.border_color(transparent_border))
        .hover(|s| s.bg(bg_hover))
        .child(render_icon(tab.icon, px(14.), icon_color))
        .child(tab.label)
}

pub fn render_tab_bar_shell(theme: &ThemeTokens) -> Div {
    let theme = theme.clone();
    div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(theme.border_subtle)
        .bg(theme.bg_surface)
}

pub fn render_tab_bar<I, F>(tabs: I, active: usize, theme: &ThemeTokens) -> impl IntoElement
where
    I: IntoIterator<Item = (TabItem, F)>,
    F: Fn(&mut App) + 'static,
{
    let theme = theme.clone();
    let mut bar = render_tab_bar_shell(&theme);

    for (index, (tab, on_click)) in tabs.into_iter().enumerate() {
        let is_active = index == active;
        bar = bar.child(
            tab_item_shell(tab, is_active, &theme)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx)),
        );
    }

    bar
}
