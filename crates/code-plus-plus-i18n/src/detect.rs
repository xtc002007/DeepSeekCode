//! Locale detection.
//!
//! Precedence, highest first:
//!
//! 1. an explicit `LanguagePref` from config or preferences (handled by the
//!    caller; [`detect_locale_full`] is only the `Auto` path),
//! 2. `CODE_PLUS_PLUS_LANG` — the CodePlusPlus-specific override, so a user can
//!    pin a language without touching their shell profile,
//! 3. `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` — the POSIX chain,
//! 4. the Windows UI language, which is the only real signal on a stock
//!    Windows install where `LANG` is unset,
//! 5. English (the caller's default when this returns `None`).

use crate::locale::Locale;

/// Environment variables consulted, in order.
pub const ENV_KEYS: [&str; 5] = [
    "CODE_PLUS_PLUS_LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_MESSAGES",
    "LANG",
];

/// Detect the locale from the environment, or `None` when nothing is expressed.
pub fn detect_locale() -> Option<Locale> {
    detect_locale_full()
}

/// Testable core of [`detect_locale`]: takes a lookup instead of touching the
/// process environment, plus the Windows answer, so every branch is reachable
/// from a unit test.
pub fn detect_with<F>(env: F, windows_ui_locale: Option<&str>) -> Option<Locale>
where
    F: Fn(&str) -> Option<String>,
{
    for key in ENV_KEYS {
        if let Some(raw) = env(key) {
            if let Some(locale) = Locale::parse(&raw) {
                return Some(locale);
            }
            // A set-but-unshipped value must not shadow a later, usable one:
            // `LANGUAGE=he` still lets `LANG=en_US` select English.
        }
    }
    windows_ui_locale.and_then(Locale::parse)
}

/// Split a POSIX locale string (`fr_FR.UTF-8`, `de:DE:2001`, `sr_RS@latin`)
/// into candidate language tags, most preferred first.
pub fn split_locale_string(raw: &str) -> Vec<String> {
    raw.split(':')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let without_modifier = part.split('@').next().unwrap_or(part);
            // A MODIFIER weight (`:1996`) is not a subtag; drop it.
            let last = without_modifier
                .rsplit(':')
                .next()
                .unwrap_or(without_modifier);
            let without_encoding = last.split('.').next().unwrap_or(last);
            without_encoding.to_string()
        })
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// Resolve a multi-valued POSIX locale string (`LANGUAGE=fr:en`) to the first
/// shipped candidate.
pub fn detect_locale_from_locale_string(raw: &str) -> Option<Locale> {
    split_locale_string(raw)
        .iter()
        .find_map(|tag| Locale::parse(tag))
}

/// Minimal Win32 declarations for the UI-language lookup.
///
/// Declared here rather than pulled from `windows-sys` so this crate keeps a
/// zero-system-dependency profile on every non-Windows build and does not take
/// on a heavyweight dependency for two functions.
#[cfg(windows)]
mod win {
    extern "system" {
        /// `LANGID` of the user's UI language. Zero means "no user language".
        pub fn GetUserDefaultUILanguage() -> u16;
        /// Writes the locale name (a BCP-47 tag) into a UTF-16 buffer.
        pub fn GetLocaleInfoW(locale: u16, lcctype: u32, buffer: *mut u16, len: i32) -> i32;
    }
    /// `LOCALE_NAME`: the BCP-47 form of a LANGID.
    pub const LOCALE_NAME: u32 = 0x0000_0006;
}

/// The Windows UI language as a BCP-47 string, or `None` off Windows.
#[cfg(windows)]
pub fn windows_ui_locale() -> Option<String> {
    // SAFETY: `GetUserDefaultUILanguage` takes no arguments and cannot fail;
    // the LANGID is only used as a non-zero sentinel.
    let lang_id = unsafe { win::GetUserDefaultUILanguage() };
    if lang_id == 0 {
        return None;
    }
    // `LOCALE_NAME` needs at most 85 UTF-16 code units including the NUL.
    let mut buf = [0u16; 85];
    // SAFETY: `buf` is a valid, writable array and `len` matches its capacity,
    // which is the contract `GetLocaleInfoW` requires.
    let len = unsafe {
        win::GetLocaleInfoW(
            lang_id,
            win::LOCALE_NAME,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// The Windows UI language, or `None` off Windows.
#[cfg(not(windows))]
pub fn windows_ui_locale() -> Option<String> {
    None
}

/// Detect the locale from the process environment and the OS.
pub fn detect_locale_full() -> Option<Locale> {
    detect_with(
        |key| std::env::var(key).ok(),
        windows_ui_locale().as_deref(),
    )
}

/// Where a detected locale came from, for diagnostics bundles and the Settings
/// "detected language" row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionReport {
    /// The environment variable or platform API that answered.
    pub source: String,
    /// The raw value it answered with.
    pub raw: String,
    /// The locale it mapped to, if it maps to one we ship.
    pub locale: Option<Locale>,
}

impl DetectionReport {
    /// Resolve the report to a usable locale, falling back to `default`.
    pub fn resolve_or(&self, default: Locale) -> Locale {
        self.locale.unwrap_or(default)
    }
}

/// What detection resolved right now.
pub fn detection_report() -> DetectionReport {
    for key in ENV_KEYS {
        if let Ok(raw) = std::env::var(key) {
            if let Some(locale) = Locale::parse(&raw) {
                return DetectionReport {
                    source: key.to_string(),
                    raw,
                    locale: Some(locale),
                };
            }
        }
    }
    match windows_ui_locale() {
        Some(raw) => {
            let locale = Locale::parse(&raw);
            DetectionReport {
                source: "windows".to_string(),
                raw,
                locale,
            }
        }
        None => DetectionReport {
            source: "default".to_string(),
            raw: "en".to_string(),
            locale: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn code_plus_plus_lang_wins_over_the_posix_chain() {
        let locale = detect_with(
            env(&[
                ("CODE_PLUS_PLUS_LANG", "ja"),
                ("LC_ALL", "de_DE.UTF-8"),
                ("LANG", "fr_FR"),
            ]),
            None,
        );
        assert_eq!(locale, Some(Locale::Ja));
    }

    #[test]
    fn the_posix_chain_is_ordered() {
        assert_eq!(
            detect_with(env(&[("LANGUAGE", "ko"), ("LANG", "de")]), None),
            Some(Locale::Ko)
        );
        assert_eq!(
            detect_with(env(&[("LC_ALL", "es_ES"), ("LC_MESSAGES", "ru_RU")]), None),
            Some(Locale::Es)
        );
        assert_eq!(
            detect_with(env(&[("LC_MESSAGES", "ru_RU"), ("LANG", "de_DE")]), None),
            Some(Locale::Ru)
        );
        assert_eq!(
            detect_with(env(&[("LANG", "de_DE")]), None),
            Some(Locale::De)
        );
    }

    #[test]
    fn an_unshipped_env_value_does_not_shadow_a_usable_one() {
        assert_eq!(
            detect_with(env(&[("LANGUAGE", "it_IT"), ("LANG", "ja_JP")]), None),
            Some(Locale::Ja)
        );
    }

    #[test]
    fn the_os_setting_is_the_last_resort() {
        assert_eq!(detect_with(env(&[]), Some("zh-TW")), Some(Locale::ZhHant));
        // A POSIX env value beats the OS setting.
        assert_eq!(
            detect_with(env(&[("LANG", "en_US")]), Some("zh-TW")),
            Some(Locale::En)
        );
        // An OS setting we do not ship resolves to `None`; callers fall back.
        assert_eq!(detect_with(env(&[]), Some("it-IT")), None);
    }

    #[test]
    fn nothing_expressed_resolves_to_none() {
        assert_eq!(detect_with(env(&[]), None), None);
        assert_eq!(detect_with(env(&[("LANG", "")]), None), None);
    }

    #[test]
    fn multi_valued_locale_strings_pick_the_first_shipped_candidate() {
        assert_eq!(
            detect_locale_from_locale_string("he:fr:en"),
            Some(Locale::Fr)
        );
        assert_eq!(detect_locale_from_locale_string("en"), Some(Locale::En));
        assert_eq!(detect_locale_from_locale_string("it:he"), None);
        assert_eq!(
            detect_locale_from_locale_string("de:DE:1996"),
            Some(Locale::De)
        );
        assert_eq!(detect_locale_from_locale_string(""), None);
    }

    #[test]
    fn locale_strings_are_split_into_clean_tags() {
        assert_eq!(
            split_locale_string("fr_FR.UTF-8:en_US"),
            vec!["fr_FR".to_string(), "en_US".to_string()]
        );
        assert_eq!(
            split_locale_string("sr_RS@latin"),
            vec!["sr_RS".to_string()]
        );
        assert!(split_locale_string("::").is_empty());
    }

    #[test]
    fn detection_report_resolves_to_a_usable_locale() {
        let report = DetectionReport {
            source: "LANG".to_string(),
            raw: "de_DE".to_string(),
            locale: Some(Locale::De),
        };
        assert_eq!(report.source, "LANG");
        assert_eq!(report.resolve_or(Locale::En), Locale::De);

        let unknown = DetectionReport {
            source: "LANG".to_string(),
            raw: "it_IT".to_string(),
            locale: None,
        };
        assert_eq!(unknown.resolve_or(Locale::Ja), Locale::Ja);
    }
}
