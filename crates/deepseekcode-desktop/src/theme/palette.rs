use gpui::*;

pub fn hex_rgb(hex: u32) -> Rgba {
    rgb(hex)
}

pub fn hex_hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    hsla(h, s, l, a)
}
