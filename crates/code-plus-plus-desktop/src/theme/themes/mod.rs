pub mod dark;
pub mod light;

use super::system;
use super::tokens::ThemeTokens;
use crate::preferences::ThemeId;

pub fn tokens_for(id: ThemeId) -> ThemeTokens {
    match id {
        ThemeId::Dark => dark::dark_tokens(),
        ThemeId::Light => light::light_tokens(),
        ThemeId::System => {
            if system::prefers_dark() {
                dark::dark_tokens()
            } else {
                light::light_tokens()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_and_light_tokens_differ() {
        let dark = dark::dark_tokens();
        let light = light::light_tokens();
        assert_ne!(dark.bg_base, light.bg_base);
        assert_ne!(dark.text_primary, light.text_primary);
    }

    #[test]
    fn system_resolves_to_dark_or_light() {
        let tokens = tokens_for(ThemeId::System);
        let dark = dark::dark_tokens();
        let light = light::light_tokens();
        assert!(
            tokens.bg_base == dark.bg_base || tokens.bg_base == light.bg_base,
            "system theme must resolve to dark or light palette"
        );
    }
}
