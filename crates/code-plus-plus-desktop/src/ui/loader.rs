//! Animated loader icon (pulses on repaint).

use crate::theme::ThemeTokens;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::*;

pub fn render_loader(theme: &ThemeTokens, size: Pixels) -> impl IntoElement {
    let theme = theme.clone();
    // P2: monotonic clock (SystemTime jumps on NTP/sleep) + neutral color
    // (warning amber read as an alert in neutral loading spots).
    // Note: pulse advances on parent repaint; the session timeline already
    // repaints every 120ms while loading (schedule_session_loading_repaint),
    // so no per-loader timer is needed (which would need &mut cx).
    let tick = std::time::Instant::now().elapsed().as_millis();
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let step = ((tick / 120 + wall / 120) / 2) % 8;
    let opacity = (0.55 + (step as f32 * 0.05)).min(0.95);
    render_icon_rgba(IconName::Loader, size, theme.text_muted.opacity(opacity))
}
