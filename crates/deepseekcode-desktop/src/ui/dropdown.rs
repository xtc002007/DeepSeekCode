//! Simple popover dropdown for Composer controls.

use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon, IconName};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DropDirection {
    #[default]
    Down,
    Up,
}

#[derive(Clone, Debug)]
pub struct DropdownOption {
    pub id: String,
    pub label: String,
    pub disabled: bool,
}

pub struct DropdownView;

impl DropdownView {
    pub fn render_trigger(
        label: impl Into<SharedString>,
        open: bool,
        direction: DropDirection,
        on_toggle: impl Fn(
                &mut crate::workspace::WorkspaceView,
                Point<Pixels>,
                &mut Context<crate::workspace::WorkspaceView>,
            ) + 'static,
        cx: &mut Context<crate::workspace::WorkspaceView>,
        theme: &ThemeTokens,
    ) -> impl IntoElement {
        let label = label.into();
        let theme = theme.clone();
        let chevron = match direction {
            DropDirection::Down => {
                if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                }
            }
            DropDirection::Up => {
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronUp
                }
            }
        };
        let bg = if open {
            theme.bg_selected
        } else {
            theme.bg_elevated
        };

        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(theme.border_subtle)
            .text_xs()
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, {
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    on_toggle(this, event.position, cx);
                })
            })
            .child(label)
            .child(render_icon(chevron, px(12.), theme.text_muted))
    }
}
