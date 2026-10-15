mod palette;
mod radius;
mod spacing;
mod style;
mod system;
pub mod themes;
mod tokens;
mod typography;

pub use radius::RadiusScale;
pub use spacing::SpacingScale;
pub use style::{font_mono, font_ui, leading_base, leading_relaxed, leading_sm, leading_xs};
pub use tokens::ThemeTokens;
pub use typography::TypographyScale;

use crate::preferences::{load_preferences, save_preferences, AppPreferences, ThemeId};
use gpui::*;

pub struct ThemeStore {
    pub active: ThemeId,
    pub tokens: ThemeTokens,
}

impl Global for ThemeStore {}

pub struct CodePlusPlusTheme;

impl CodePlusPlusTheme {
    pub fn init(cx: &mut App) {
        let prefs = load_preferences();
        let tokens = themes::tokens_for(prefs.theme);
        cx.set_global(ThemeStore {
            active: prefs.theme,
            tokens,
        });
    }

    pub fn tokens(cx: &App) -> ThemeTokens {
        cx.global::<ThemeStore>().tokens.clone()
    }

    pub fn tokens_from_context<T>(cx: &Context<T>) -> ThemeTokens {
        cx.read_global(|store: &ThemeStore, _| store.tokens.clone())
    }

    pub fn theme_id(cx: &App) -> ThemeId {
        cx.global::<ThemeStore>().active
    }

    pub fn switch(cx: &mut App, id: ThemeId) {
        let tokens = themes::tokens_for(id);
        cx.update_global::<ThemeStore, _>(|store, _| {
            store.active = id;
            store.tokens = tokens;
        });
    }

    pub fn switch_and_save(cx: &mut App, id: ThemeId, prefs: &mut AppPreferences) {
        prefs.theme = id;
        save_preferences(prefs);
        Self::switch(cx, id);
    }
}

pub fn accent_from(cx: &App) -> Hsla {
    cx.global::<ThemeStore>().tokens.accent
}
