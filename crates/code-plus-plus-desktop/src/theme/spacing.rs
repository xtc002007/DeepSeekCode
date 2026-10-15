use gpui::px;
use gpui::Pixels;

/// Spacing scale — tightened for IDE-like density (Cursor-aligned).
#[derive(Clone, Copy, Debug)]
pub struct SpacingScale {
    pub space_1: Pixels,
    pub space_2: Pixels,
    pub space_3: Pixels,
    pub space_4: Pixels,
    pub space_5: Pixels,
    pub space_6: Pixels,
    pub space_7: Pixels,
    pub space_8: Pixels,
}

impl Default for SpacingScale {
    fn default() -> Self {
        Self {
            space_1: px(3.),
            space_2: px(6.),
            space_3: px(8.),
            space_4: px(12.),
            space_5: px(16.),
            space_6: px(20.),
            space_7: px(28.),
            space_8: px(36.),
        }
    }
}
