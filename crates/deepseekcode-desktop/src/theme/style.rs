//! Theme-aware styling helpers for views.

use super::tokens::ThemeTokens;
use gpui::*;

pub fn font_ui(theme: &ThemeTokens) -> SharedString {
    theme.typography.font_ui.clone()
}

pub fn font_mono(theme: &ThemeTokens) -> SharedString {
    theme.typography.font_mono.clone()
}

pub fn leading_xs(theme: &ThemeTokens) -> Pixels {
    theme.typography.leading_xs
}

pub fn leading_sm(theme: &ThemeTokens) -> Pixels {
    theme.typography.leading_sm
}

pub fn leading_base(theme: &ThemeTokens) -> Pixels {
    theme.typography.leading_base
}

pub fn leading_relaxed(theme: &ThemeTokens) -> Pixels {
    theme.typography.leading_relaxed
}

/// Elevated surface styling (light theme gets a subtle shadow).
pub fn elevated_surface(theme: &ThemeTokens) -> impl Fn(Div) -> Div + Clone {
    let shadow = theme.shadow_elevated;
    let bg = theme.bg_elevated;
    move |el: Div| {
        let el = el.bg(bg);
        if let Some(shadow) = shadow {
            el.shadow(vec![BoxShadow {
                color: shadow,
                offset: point(px(0.), px(1.)),
                blur_radius: px(4.),
                spread_radius: px(0.),
                inset: false,
            }])
        } else {
            el
        }
    }
}

/// Focus ring for interactive controls.
pub fn focus_ring(theme: &ThemeTokens, focused: bool) -> impl Fn(Stateful<Div>) -> Stateful<Div> + Clone {
    let border_focus = theme.border_focus;
    let transparent: Rgba = hsla(0., 0., 0., 0.).into();
    move |el: Stateful<Div>| {
        el.border_1().border_color(if focused {
            border_focus
        } else {
            transparent
        })
    }
}
