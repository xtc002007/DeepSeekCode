//! Animated loader icon (pulses on repaint).

use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub fn render_loader(theme: &ThemeTokens, size: Pixels) -> impl IntoElement {
    let theme = theme.clone();
    let phase = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() / 120) % 8)
        .unwrap_or(0);
    let opacity = 0.35 + (phase as f32 * 0.08);
    render_icon_rgba(IconName::Loader, size, theme.warning.opacity(opacity))
}
