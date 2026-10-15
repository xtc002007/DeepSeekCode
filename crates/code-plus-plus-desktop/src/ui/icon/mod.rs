include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));

pub mod file_type;

use gpui::*;

pub fn render_icon(name: IconName, size: Pixels, color: impl Into<Hsla>) -> impl IntoElement {
    let color = color.into();
    svg()
        .flex_none()
        .flex_shrink_0()
        .size(size)
        .text_color(color)
        .path(name.path())
}

pub fn render_icon_rgba(name: IconName, size: Pixels, color: Rgba) -> impl IntoElement {
    render_icon(name, size, color)
}
