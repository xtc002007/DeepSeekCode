use gpui::Pixels;
use gpui::px;

#[derive(Clone, Copy, Debug)]
pub struct RadiusScale {
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
}

impl Default for RadiusScale {
    fn default() -> Self {
        Self {
            sm: px(4.),
            md: px(6.),
            lg: px(8.),
        }
    }
}
