//! Locale identity: the supported locale set, user preference, and BCP-47
//! tag parsing.
//!
//! Ten mainstream locales ship in the box (see [`Locale::ALL`]). Everything the
//! rest of the crate needs to answer — canonical tag, endonym, script/region,
//! text direction — is derived from this enum, so adding a locale is a
//! single-variant change plus a catalog file.

use serde::{Deserialize, Serialize};

/// Text direction. None of the ten shipped locales is RTL, but the enum exists
/// so the layout layer can branch on it without a special case for every call
/// site once one is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Left-to-right base direction: the script runs left to right.
    Ltr,
    /// Right-to-left base direction: the script runs right to left.
    Rtl,
}

/// One of the locales Code++ ships translations for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Locale {
    /// English. The source language and the fallback for every catalog.
    #[serde(rename = "en")]
    En,
    /// Simplified Chinese (Hans script).
    #[serde(rename = "zh-Hans")]
    ZhHans,
    /// Traditional Chinese (Hant script).
    #[serde(rename = "zh-Hant")]
    ZhHant,
    /// Japanese.
    #[serde(rename = "ja")]
    Ja,
    /// Korean.
    #[serde(rename = "ko")]
    Ko,
    /// Spanish.
    #[serde(rename = "es")]
    Es,
    /// French.
    #[serde(rename = "fr")]
    Fr,
    /// German.
    #[serde(rename = "de")]
    De,
    /// Brazilian Portuguese. Also selected for European Portuguese, for which
    /// no separate catalog ships.
    #[serde(rename = "pt-BR")]
    PtBr,
    /// Russian.
    #[serde(rename = "ru")]
    Ru,
}

impl Default for Locale {
    /// English is the source language: every catalog falls back to it, so it is
    /// the only correct default when detection has nothing to say.
    fn default() -> Self {
        Locale::En
    }
}

impl Locale {
    /// Every shipped locale, in the order the Settings language picker lists
    /// them. The order is intentional (English first, then script families,
    /// then Latin-script European languages) and is asserted by tests.
    pub const ALL: [Locale; 10] = [
        Locale::En,
        Locale::ZhHans,
        Locale::ZhHant,
        Locale::Ja,
        Locale::Ko,
        Locale::Es,
        Locale::Fr,
        Locale::De,
        Locale::PtBr,
        Locale::Ru,
    ];

    /// Canonical BCP-47 tag. Also the on-disk config/config-file spelling and
    /// the `Accept-Language` tag this locale answers to.
    pub const fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::ZhHans => "zh-Hans",
            Locale::ZhHant => "zh-Hant",
            Locale::Ja => "ja",
            Locale::Ko => "ko",
            Locale::Es => "es",
            Locale::Fr => "fr",
            Locale::De => "de",
            Locale::PtBr => "pt-BR",
            Locale::Ru => "ru",
        }
    }

    /// Locale for the `Content-Language` / HTML `lang` value: the tag plus the
    /// region a monolingual form implies, so downstream consumers (assistive
    /// tech, hyphenation, speech engines) get a fully-specified tag.
    pub const fn html_lang(self) -> &'static str {
        match self {
            Locale::En => "en-US",
            Locale::ZhHans => "zh-Hans-CN",
            Locale::ZhHant => "zh-Hant-TW",
            Locale::Ja => "ja-JP",
            Locale::Ko => "ko-KR",
            Locale::Es => "es-ES",
            Locale::Fr => "fr-FR",
            Locale::De => "de-DE",
            Locale::PtBr => "pt-BR",
            Locale::Ru => "ru-RU",
        }
    }

    /// The language's name in that language, for the language picker. Never
    /// localized: an endonym must be readable in the picker regardless of the
    /// currently selected locale.
    pub const fn endonym(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::ZhHans => "简体中文",
            Locale::ZhHant => "繁體中文",
            Locale::Ja => "日本語",
            Locale::Ko => "한국어",
            Locale::Es => "Español",
            Locale::Fr => "Français",
            Locale::De => "Deutsch",
            Locale::PtBr => "Português (Brasil)",
            Locale::Ru => "Русский",
        }
    }

    /// English name, used in diagnostics bundles and logs where the reading
    /// audience is not the end user.
    pub const fn english_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::ZhHans => "Chinese (Simplified)",
            Locale::ZhHant => "Chinese (Traditional)",
            Locale::Ja => "Japanese",
            Locale::Ko => "Korean",
            Locale::Es => "Spanish",
            Locale::Fr => "French",
            Locale::De => "German",
            Locale::PtBr => "Portuguese (Brazil)",
            Locale::Ru => "Russian",
        }
    }

    /// True for the CJK locales, which need wide-glyph handling in layout.
    pub const fn is_cjk(self) -> bool {
        matches!(
            self,
            Locale::ZhHans | Locale::ZhHant | Locale::Ja | Locale::Ko
        )
    }

    /// Base text direction for the locale.
    pub const fn direction(self) -> Direction {
        match self {
            // No RTL locale ships today; the match is exhaustive so adding
            // one becomes a compile error here rather than a silent wrong
            // layout.
            Locale::En
            | Locale::ZhHans
            | Locale::ZhHant
            | Locale::Ja
            | Locale::Ko
            | Locale::Es
            | Locale::Fr
            | Locale::De
            | Locale::PtBr
            | Locale::Ru => Direction::Ltr,
        }
    }

    /// Parse a user- or system-supplied tag into a shipped locale.
    ///
    /// Accepts, in order of specificity:
    /// - canonical tags and their case variants: `zh-Hans`, `zh_Hant`, `PT-br`
    /// - POSIX locale strings: `zh_CN.UTF-8`, `de_DE@euro` (encoding and
    ///   modifier stripped)
    /// - bare language subtags: `ja`, `ru`
    /// - Windows LANGID-derived names: `zh-CN`, `zh-HK`, `zh-MO`
    /// - human display names and endonyms: `Chinese (Traditional)`, `繁體中文`,
    ///   `日本語`, `Português`
    ///
    /// Returns `None` for anything unshipped so callers can distinguish "user
    /// asked for a language we do not have" (warn, keep English) from "no
    /// preference expressed" (silent English).
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // Strip a POSIX encoding (`.UTF-8`) and modifier (`@euro`), then
        // normalize separators and case for prefix matching.
        let mut base = raw.split('@').next().unwrap_or(raw);
        if let Some(dot) = base.find('.') {
            base = &base[..dot];
        }
        // `sr_Latn_RS@latin`: keep only the first two subtags, which is all a
        // language match can meaningfully use.
        let parts: Vec<&str> = base.split(['-', '_']).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return None;
        }
        // Drop a Unicode-locale extension subtag (`de-DE-u-co-phonebk`).
        let mut subtags: Vec<String> = Vec::with_capacity(3);
        for (idx, part) in parts.iter().enumerate() {
            let lower = part.to_ascii_lowercase();
            if lower == "u" || lower == "t" {
                break;
            }
            if idx > 1 {
                break;
            }
            subtags.push(lower);
        }
        let lang = subtags.first().cloned().unwrap_or_default();
        let rest: Vec<&str> = subtags[1..].iter().map(String::as_str).collect();

        // Endonyms / English display names: match on the whole input before the
        // subtag logic so `Chinese (Traditional)` does not collapse to `zh`.
        let lowered = raw.to_lowercase();
        if let Some(locale) = match_endonym(&lowered) {
            return Some(locale);
        }

        match lang.as_str() {
            "zh" => {
                // Traditional markers: explicit script, or a traditional region.
                // `zh-HK`/`zh-MO` are traditional in practice even without the
                // `Hant` subtag; `zh-SG`/`zh-MY` are simplified.
                let traditional = rest.iter().any(|s| {
                    matches!(
                        *s,
                        "hant" | "tw" | "hk" | "mo" | "繁體" | "繁体" | "traditional"
                    )
                });
                let simplified = rest
                    .iter()
                    .any(|s| matches!(*s, "hans" | "cn" | "sg" | "my" | "简体"));
                match (traditional, simplified) {
                    (true, false) => Some(Locale::ZhHant),
                    (false, true) => Some(Locale::ZhHans),
                    // Both markers, or neither (a bare `zh`): Simplified is
                    // the safer default — it matches the Windows zh-CN
                    // default and the larger share of `zh` deployments.
                    _ => Some(Locale::ZhHans),
                }
            }
            "ja" | "jpn" => Some(Locale::Ja),
            "ko" | "kor" => Some(Locale::Ko),
            "es" | "spa" => Some(Locale::Es),
            "fr" | "fra" | "fre" => Some(Locale::Fr),
            "de" | "deu" | "ger" => Some(Locale::De),
            // One Portuguese: `pt-PT` maps to the Brazilian catalog rather than
            // silently falling back to English.
            "pt" | "por" => Some(Locale::PtBr),
            "ru" | "rus" => Some(Locale::Ru),
            "en" | "eng" => Some(Locale::En),
            _ => None,
        }
    }

    /// Best-effort match for a whole `Accept-Language` header value.
    ///
    /// Quality values are honoured, then order, then a second pass over the
    /// languages whose *base* subtag matches a shipped locale even if the full
    /// tag did not (so `en-GB` still selects English and `de-AT` still selects
    /// German).
    pub fn parse_accept_language(header: &str) -> Option<Self> {
        let mut candidates: Vec<(f32, usize, String)> = Vec::new();
        for (index, part) in header.split(',').enumerate() {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let mut bits = part.split(';');
            let tag = bits.next().unwrap_or("").trim().to_string();
            if tag.is_empty() || tag == "*" {
                continue;
            }
            let mut q = 1.0f32;
            for param in bits {
                let param = param.trim();
                if let Some(value) = param.strip_prefix("q=") {
                    q = value.parse().unwrap_or(0.0);
                }
            }
            candidates.push((q, index, tag));
        }
        // Highest q first; ties keep header order (stable, not reversed).
        candidates.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        for (_, _, tag) in &candidates {
            if let Some(locale) = Locale::parse(tag) {
                return Some(locale);
            }
        }
        // Second pass: match on the primary subtag only.
        for (_, _, tag) in &candidates {
            let base = tag.split(['-', '_']).next().unwrap_or(tag);
            if let Some(locale) = Locale::parse(base) {
                return Some(locale);
            }
        }
        None
    }
}

/// Lowercase and drop every non-alphanumeric character, so that
/// `Chinese (Traditional, Taiwan)`, `chinese_traditional` and
/// `Chinese Traditional` all reduce to the same query.
fn compact(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_alphanumeric() {
            for lowered in c.to_lowercase() {
                out.push(lowered);
            }
        }
    }
    out
}

/// Resolve a human-written language name to a shipped locale.
///
/// Accepts, in order:
/// - a canonical tag, case-insensitively (`zh-hant`, `PT-br`);
/// - an exact endonym or English name (`鏃ユ湰瑾瀈, `Deutsch`);
/// - a decorated name that *starts with* one of those
///   (`Chinese (Traditional, Taiwan)` 鈫?`Chinese (Traditional)`);
/// - a name that a shipped name *starts with* (`Portugu锚s` for
///   `Portugu锚s (Brasil)`), but only when that is unambiguous.
///
/// Short fragments are rejected: `it` is a substring of `traditional` but
/// is Italian, not Chinese, and a substring test would get that backwards.
fn match_endonym(raw: &str) -> Option<Locale> {
    let query = compact(raw);
    if query.len() < 3 {
        return None;
    }
    // A bare "Chinese" does not say which script; let the subtag logic pick the
    // Simplified default rather than guessing here.
    if query == "chinese" {
        return None;
    }
    let mut forward: Option<(usize, Locale)> = None;
    let mut reverse: Vec<(usize, Locale)> = Vec::new();
    for locale in Locale::ALL {
        if compact(locale.tag()) == query {
            return Some(locale);
        }
        for name in [locale.english_name(), locale.endonym()] {
            let candidate = compact(name);
            if candidate.is_empty() || candidate.len() < 3 {
                continue;
            }
            if candidate == query {
                return Some(locale);
            }
            if query.starts_with(&candidate) {
                // Chinese (Traditional, Taiwan) 鈫?Chinese (Traditional).
                // Prefer the longest candidate so a decorated Chinese name can
                // never collapse onto the wrong script.
                if forward.is_none_or(|(len, _)| candidate.len() > len) {
                    forward = Some((candidate.len(), locale));
                }
            } else if candidate.starts_with(&query) && query.len() >= 4 {
                // Portugu锚s 鈫?Portugu锚s (Brasil).
                if !reverse.iter().any(|(_, seen)| *seen == locale) {
                    reverse.push((candidate.len(), locale));
                }
            }
        }
    }
    if let Some((_, locale)) = forward {
        return Some(locale);
    }
    // A reverse match that resolves to two different locales is a genuine
    // ambiguity (chin); report nothing and let the caller fall back.
    match reverse.as_slice() {
        [(_, locale)] => Some(*locale),
        [] => None,
        _ => None,
    }
}
/// User-facing language preference: follow the environment, or pin a locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguagePref {
    /// Detect from the environment (`CODE_PLUS_PLUS_LANG`, `LANG`, the OS
    /// locale). The default: a fresh install should speak the user's language
    /// without them configuring anything.
    #[default]
    Auto,
    /// A pinned locale, stored as its canonical BCP-47 tag.
    #[serde(untagged)]
    Locale(Locale),
}

impl LanguagePref {
    /// Parse a config/preferences value. `"auto"`, `""` and `null` all mean
    /// detect; anything else is a tag passed to [`Locale::parse`].
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return LanguagePref::Auto;
        }
        match Locale::parse(trimmed) {
            Some(locale) => LanguagePref::Locale(locale),
            None => LanguagePref::Auto,
        }
    }

    /// Resolve to a concrete locale, detecting one when this is `Auto`.
    pub fn resolve(self, fallback: Locale) -> Locale {
        match self {
            LanguagePref::Locale(locale) => locale,
            LanguagePref::Auto => crate::detect::detect_locale().unwrap_or(fallback),
        }
    }

    /// The canonical string written to config/preferences: `"auto"` or a tag.
    pub fn as_str(self) -> &'static str {
        match self {
            LanguagePref::Auto => "auto",
            LanguagePref::Locale(locale) => locale.tag(),
        }
    }

    /// `Auto` followed by every shipped locale, for a Settings picker.
    pub const fn all() -> [LanguagePref; 11] {
        [
            LanguagePref::Auto,
            LanguagePref::Locale(Locale::En),
            LanguagePref::Locale(Locale::ZhHans),
            LanguagePref::Locale(Locale::ZhHant),
            LanguagePref::Locale(Locale::Ja),
            LanguagePref::Locale(Locale::Ko),
            LanguagePref::Locale(Locale::Es),
            LanguagePref::Locale(Locale::Fr),
            LanguagePref::Locale(Locale::De),
            LanguagePref::Locale(Locale::PtBr),
            LanguagePref::Locale(Locale::Ru),
        ]
    }
}

impl From<Option<Locale>> for LanguagePref {
    fn from(value: Option<Locale>) -> Self {
        value.map_or(LanguagePref::Auto, LanguagePref::Locale)
    }
}

impl From<LanguagePref> for Option<Locale> {
    fn from(value: LanguagePref) -> Self {
        match value {
            LanguagePref::Auto => None,
            LanguagePref::Locale(locale) => Some(locale),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_tags_round_trip() {
        for locale in Locale::ALL {
            assert_eq!(Locale::parse(locale.tag()), Some(locale));
            assert_eq!(Locale::parse(&locale.tag().to_uppercase()), Some(locale));
            assert_eq!(
                LanguagePref::parse(locale.tag()),
                LanguagePref::Locale(locale)
            );
        }
    }

    #[test]
    fn posix_locale_strings_are_understood() {
        assert_eq!(Locale::parse("zh_CN.UTF-8"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh_TW.UTF-8"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("de_DE.UTF-8@euro"), Some(Locale::De));
        assert_eq!(Locale::parse("fr_FR"), Some(Locale::Fr));
        assert_eq!(Locale::parse("pt_BR.UTF-8"), Some(Locale::PtBr));
        assert_eq!(Locale::parse("pt_PT"), Some(Locale::PtBr));
    }

    #[test]
    fn windows_locale_names_map_to_the_right_chinese_variant() {
        assert_eq!(Locale::parse("zh-CN"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh-SG"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh-TW"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("zh-HK"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("zh-MO"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("zh"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh-Hant"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("zh-Hans"), Some(Locale::ZhHans));
        // Script subtag must win over region order (`zh-TW-Hant`).
        assert_eq!(Locale::parse("zh-TW-Hant"), Some(Locale::ZhHant));
    }

    #[test]
    fn unicode_extension_subtags_are_ignored() {
        assert_eq!(Locale::parse("de-DE-u-co-phonebk"), Some(Locale::De));
        assert_eq!(Locale::parse("sr-Latn-RS@latin"), None);
    }

    #[test]
    fn endonyms_and_display_names_are_accepted() {
        assert_eq!(Locale::parse("日本語"), Some(Locale::Ja));
        assert_eq!(Locale::parse("繁體中文"), Some(Locale::ZhHant));
        assert_eq!(Locale::parse("简体中文"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("Français"), Some(Locale::Fr));
        assert_eq!(Locale::parse("Português"), Some(Locale::PtBr));
        assert_eq!(
            Locale::parse("Chinese (Traditional, Taiwan)"),
            Some(Locale::ZhHant)
        );
        assert_eq!(
            Locale::parse("Chinese (Simplified, China)"),
            Some(Locale::ZhHans)
        );
        assert_eq!(Locale::parse("Russian"), Some(Locale::Ru));
    }

    #[test]
    fn unshipped_languages_return_none() {
        assert_eq!(Locale::parse("it"), None);
        assert_eq!(Locale::parse("nl-NL"), None);
        assert_eq!(Locale::parse(""), None);
        assert_eq!(Locale::parse("   "), None);
        assert_eq!(Locale::parse("*"), None);
    }

    #[test]
    fn accept_language_honours_quality_then_order() {
        assert_eq!(
            Locale::parse_accept_language("de-DE,de;q=0.9"),
            Some(Locale::De)
        );
        assert_eq!(
            Locale::parse_accept_language("fr;q=0.2,ja;q=0.9"),
            Some(Locale::Ja)
        );
        assert_eq!(
            Locale::parse_accept_language("it;q=1.0,ko;q=0.8"),
            Some(Locale::Ko)
        );
        assert_eq!(Locale::parse_accept_language("*"), None);
        assert_eq!(Locale::parse_accept_language(""), None);
        // Second pass: region-specific tag for an unshipped region still maps.
        assert_eq!(Locale::parse_accept_language("en-GB"), Some(Locale::En));
        assert_eq!(Locale::parse_accept_language("de-AT"), Some(Locale::De));
    }

    #[test]
    fn language_pref_defaults_to_auto() {
        assert_eq!(LanguagePref::default(), LanguagePref::Auto);
        assert_eq!(LanguagePref::parse(""), LanguagePref::Auto);
        assert_eq!(LanguagePref::parse("auto"), LanguagePref::Auto);
        assert_eq!(LanguagePref::parse("AUTO"), LanguagePref::Auto);
        assert_eq!(LanguagePref::parse("nonsense"), LanguagePref::Auto);
        assert_eq!(LanguagePref::Auto.as_str(), "auto");
        assert_eq!(LanguagePref::Locale(Locale::Ja).as_str(), "ja");
        assert_eq!(LanguagePref::all().len(), 11);
    }

    #[test]
    fn language_pref_serializes_as_auto_or_tag() {
        let json = serde_json::to_string(&LanguagePref::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
        let json = serde_json::to_string(&LanguagePref::Locale(Locale::PtBr)).unwrap();
        assert_eq!(json, "\"pt-BR\"");
        let parsed: LanguagePref = serde_json::from_str("\"zh-Hant\"").unwrap();
        assert_eq!(parsed, LanguagePref::Locale(Locale::ZhHant));
    }

    #[test]
    fn every_locale_has_distinct_metadata() {
        let mut tags: Vec<&str> = Locale::ALL.iter().map(|l| l.tag()).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), Locale::ALL.len(), "duplicate canonical tag");

        let mut html: Vec<&str> = Locale::ALL.iter().map(|l| l.html_lang()).collect();
        html.sort_unstable();
        html.dedup();
        assert_eq!(html.len(), Locale::ALL.len(), "duplicate html lang");

        let mut endonyms: Vec<&str> = Locale::ALL.iter().map(|l| l.endonym()).collect();
        endonyms.sort_unstable();
        endonyms.dedup();
        assert_eq!(endonyms.len(), Locale::ALL.len(), "duplicate endonym");

        let mut names: Vec<&str> = Locale::ALL.iter().map(|l| l.english_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Locale::ALL.len(), "duplicate english name");
    }

    #[test]
    fn direction_and_script_helpers() {
        assert_eq!(Locale::En.direction(), Direction::Ltr);
        assert_eq!(Locale::Ru.direction(), Direction::Ltr);
        assert!(Locale::ZhHans.is_cjk());
        assert!(Locale::Ja.is_cjk());
        assert!(!Locale::En.is_cjk());
        assert!(!Locale::Ru.is_cjk());
    }
}
