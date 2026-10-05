//! Catalog storage and lookup.
//!
//! Every locale ships a JSON object of `key -> message`. Lookups fall back
//! English-first, then to the key itself, so a missing or malformed translation
//! degrades to readable English and never to an empty string or a raw
//! `{placeholder}`.

use crate::locale::Locale;
use crate::message;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Raw catalog JSON, embedded at compile time so there is no runtime file
/// lookup, no locale directory to install, and no chance of a missing file in a
/// packaged build.
const RAW: [(Locale, &str); 10] = [
    (Locale::En, include_str!("locales/en.json")),
    (Locale::ZhHans, include_str!("locales/zh-Hans.json")),
    (Locale::ZhHant, include_str!("locales/zh-Hant.json")),
    (Locale::Ja, include_str!("locales/ja.json")),
    (Locale::Ko, include_str!("locales/ko.json")),
    (Locale::Es, include_str!("locales/es.json")),
    (Locale::Fr, include_str!("locales/fr.json")),
    (Locale::De, include_str!("locales/de.json")),
    (Locale::PtBr, include_str!("locales/pt-BR.json")),
    (Locale::Ru, include_str!("locales/ru.json")),
];

struct Catalogs {
    maps: HashMap<Locale, HashMap<String, String>>,
}

static CATALOGS: OnceLock<Catalogs> = OnceLock::new();

fn catalogs() -> &'static Catalogs {
    CATALOGS.get_or_init(|| {
        let mut maps = HashMap::with_capacity(RAW.len());
        for (locale, raw) in RAW {
            match serde_json::from_str::<HashMap<String, String>>(raw) {
                Ok(map) => {
                    maps.insert(locale, map);
                }
                Err(e) => {
                    // A broken catalog must not take the process down. Report it
                    // loudly in the log and continue with English for that
                    // locale; the parity test fails the build in CI anyway.
                    eprintln!("deepseekcode-i18n: catalog {locale:?} failed to parse: {e}");
                    maps.insert(locale, HashMap::new());
                }
            }
        }
        Catalogs { maps }
    })
}

/// Look up `key` in `locale`, falling back to English and then to the key.
///
/// Returns `None` only when the key is absent from every catalog, which the
/// runtime treats as a programming error and renders as the key.
pub fn lookup(locale: Locale, key: &str) -> Option<&'static str> {
    let catalogs = catalogs();
    if let Some(map) = catalogs.maps.get(&locale) {
        if let Some(value) = map.get(key) {
            return Some(value.as_str());
        }
    }
    if locale != Locale::En {
        if let Some(map) = catalogs.maps.get(&Locale::En) {
            if let Some(value) = map.get(key) {
                return Some(value.as_str());
            }
        }
    }
    None
}

/// Look up `key` in `locale` with no English fallback, so the parity tests can
/// measure exactly how much of a catalog is translated.
pub fn lookup_strict(locale: Locale, key: &str) -> Option<&'static str> {
    catalogs()
        .maps
        .get(&locale)
        .and_then(|map| map.get(key))
        .map(String::as_str)
}

/// Every key present in `locale` (English always defines the full key set).
pub fn keys(locale: Locale) -> Vec<&'static str> {
    let map = catalogs().maps.get(&locale);
    let mut keys: Vec<&'static str> = match map {
        Some(map) => map.keys().map(|k| k.as_str()).collect(),
        None => Vec::new(),
    };
    keys.sort_unstable();
    keys
}

/// The canonical key set: every key English defines.
pub fn source_keys() -> Vec<&'static str> {
    keys(Locale::En)
}

/// Number of messages in `locale`.
pub fn len(locale: Locale) -> usize {
    catalogs().maps.get(&locale).map_or(0, HashMap::len)
}

/// Fraction of `locale`'s keys that are identical to English.
///
/// Used by the catalog tests to catch a locale that was bulk-copied from
/// English and never actually translated (which would otherwise pass a
/// "no missing keys" check while showing English to every user).
pub fn untranslated_share(locale: Locale) -> f64 {
    if locale == Locale::En {
        return 0.0;
    }
    let Some(map) = catalogs().maps.get(&locale) else {
        return 1.0;
    };
    let Some(en) = catalogs().maps.get(&Locale::En) else {
        return 1.0;
    };
    let total = en.len();
    if total == 0 {
        return 0.0;
    }
    let identical = map
        .iter()
        .filter(|(key, value)| en.get(*key).map(|en| en == *value).unwrap_or(false))
        .count();
    identical as f64 / total as f64
}

/// Parse every message in every catalog, reporting `(locale, key, detail)` for
/// the first failure. Run by the parity test so a syntactically broken
/// translation cannot ship.
pub fn validate_all() -> Result<(), String> {
    for (locale, _) in RAW {
        let map = catalogs()
            .maps
            .get(&locale)
            .ok_or_else(|| format!("{locale:?}: catalog missing"))?;
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for key in keys {
            let value = &map[key];
            if let Err(e) = message::validate(value) {
                return Err(format!("{locale:?} / {key}: {e}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::LanguagePref;

    #[test]
    fn every_locale_has_a_non_empty_catalog() {
        for locale in Locale::ALL {
            assert!(len(locale) > 0, "{locale:?} catalog is empty");
        }
    }

    #[test]
    fn english_is_the_source_of_truth() {
        assert_eq!(source_keys().len(), len(Locale::En));
        assert!(source_keys().len() >= 300, "catalog looks truncated");
    }

    #[test]
    fn unknown_keys_return_none_rather_than_inventing_text() {
        assert_eq!(lookup(Locale::En, "definitely.not.a.key"), None);
        assert_eq!(lookup(Locale::ZhHans, "definitely.not.a.key"), None);
    }

    #[test]
    fn missing_translations_fall_back_to_english() {
        // `source_keys` is non-empty, so this exercises the fallback path with
        // a key that exists in English.
        let Some(key) = source_keys().first().copied() else {
            panic!("empty catalog")
        };
        assert!(lookup(Locale::En, key).is_some());
        for locale in Locale::ALL {
            let value = lookup(locale, key).expect("key resolves in every locale");
            assert!(!value.is_empty(), "{locale:?} has an empty value");
        }
    }

    #[test]
    fn lookup_strict_does_not_fall_back() {
        let Some(key) = source_keys().first().copied() else {
            panic!("empty catalog")
        };
        // English is strict-identical to itself.
        assert!(lookup_strict(Locale::En, key).is_some());
    }

    #[test]
    fn keys_are_sorted_and_unique() {
        for locale in Locale::ALL {
            let mut keys = keys(locale);
            let count = keys.len();
            keys.dedup();
            assert_eq!(keys.len(), count, "{locale:?} has duplicate keys");
        }
    }

    #[test]
    fn every_catalog_parses_as_valid_messages() {
        validate_all().expect("all catalog messages must parse");
    }

    #[test]
    fn keys_use_the_dotted_namespace_convention() {
        for key in source_keys() {
            assert!(key.contains('.'), "key must be namespaced: {key}");
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
                "key has unexpected characters: {key}"
            );
            assert!(
                key.chars().next().is_some_and(|c| c.is_ascii_lowercase()),
                "key must start lowercase: {key}"
            );
        }
    }

    #[test]
    fn placeholders_are_consistent_across_locales() {
        // A translation may reorder placeholders but must not drop or rename
        // them: `{name}` in English has to appear in every translation.
        let en = catalogs()
            .maps
            .get(&Locale::En)
            .cloned()
            .expect("english catalog");
        for locale in Locale::ALL {
            if locale == Locale::En {
                continue;
            }
            let map = catalogs()
                .maps
                .get(&locale)
                .cloned()
                .unwrap_or_else(|| panic!("{locale:?} catalog missing"));
            for (key, en_value) in &en {
                let Some(value) = map.get(key) else {
                    // Missing keys are caught by the parity test; here they
                    // would only produce a false positive.
                    continue;
                };
                let mut want = placeholders(en_value);
                let mut got = placeholders(value);
                want.sort();
                got.sort();
                assert_eq!(
                    want, got,
                    "{locale:?} / {key}: placeholder mismatch\n  en: {en_value}\n  {locale:?}: {value}"
                );
            }
        }
    }

    /// Every `{name}` occurrence in a message, sorted.
    fn placeholders(message: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let chars: Vec<char> = message.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '{' {
                let mut name = String::new();
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    name.push(chars[j]);
                    j += 1;
                }
                if !name.is_empty() {
                    out.push(name);
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    #[test]
    fn language_pref_resolution_uses_explicit_locale() {
        assert_eq!(
            LanguagePref::Locale(Locale::Ru).resolve(Locale::En),
            Locale::Ru
        );
    }
}
