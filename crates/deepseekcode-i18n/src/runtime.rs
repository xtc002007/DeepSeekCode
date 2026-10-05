//! The process-wide active locale and the translation entry points every
//! surface uses.
//!
//! The active locale is a single global rather than a value threaded through
//! the UI tree because three consumers need it with no common ancestor: GPUI
//! render closures, the CLI's `println!` sites, and the HTTP server's error
//! responder. Reads are behind an `RwLock`; writes happen only when the user
//! changes the language in Settings, which triggers a full re-render.

use crate::catalog;
use crate::detect;
use crate::format;
use crate::locale::{LanguagePref, Locale};
use crate::message::{self, Arg, ArgValue};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// Locale discriminant so the hot read path is a relaxed atomic load rather
/// than a lock.
const EN: u8 = 0;
const ZH_HANS: u8 = 1;
const ZH_HANT: u8 = 2;
const JA: u8 = 3;
const KO: u8 = 4;
const ES: u8 = 5;
const FR: u8 = 6;
const DE: u8 = 7;
const PT_BR: u8 = 8;
const RU: u8 = 9;

const fn discriminant(locale: Locale) -> u8 {
    match locale {
        Locale::En => EN,
        Locale::ZhHans => ZH_HANS,
        Locale::ZhHant => ZH_HANT,
        Locale::Ja => JA,
        Locale::Ko => KO,
        Locale::Es => ES,
        Locale::Fr => FR,
        Locale::De => DE,
        Locale::PtBr => PT_BR,
        Locale::Ru => RU,
    }
}

const fn from_discriminant(value: u8) -> Locale {
    match value {
        ZH_HANS => Locale::ZhHans,
        ZH_HANT => Locale::ZhHant,
        JA => Locale::Ja,
        KO => Locale::Ko,
        ES => Locale::Es,
        FR => Locale::Fr,
        DE => Locale::De,
        PT_BR => Locale::PtBr,
        RU => Locale::Ru,
        _ => Locale::En,
    }
}

static ACTIVE: AtomicU8 = AtomicU8::new(EN);

/// The reason a locale change happened, so callers can react appropriately
/// (re-render the whole shell vs. only rebuilding clap help).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleChange {
    /// The first resolution at process start.
    Initial,
    /// The user changed the preference.
    User,
}

/// Listeners notified after the active locale changes. The desktop registers
/// one to invalidate its cached strings and repaint.
type Listener = Box<dyn Fn(Locale, LocaleChange) + Send + Sync + 'static>;

fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a callback invoked after the active locale changes.
///
/// Returns a guard-free registration: the listener lives for the process. This
/// is deliberate — a UI shell has exactly one lifetime and an unsubscribe path
/// that can silently leak would be worse than a permanent registration.
pub fn on_locale_change<F>(listener: F)
where
    F: Fn(Locale, LocaleChange) + Send + Sync + 'static,
{
    if let Ok(mut guard) = listeners().lock() {
        guard.push(Box::new(listener));
    }
}

/// The locale currently used for rendering.
#[inline]
pub fn locale() -> Locale {
    from_discriminant(ACTIVE.load(Ordering::Relaxed))
}

/// Resolve a preference to a concrete locale, detecting for `Auto`.
pub fn resolve(pref: LanguagePref) -> Locale {
    pref.resolve(Locale::En)
}

/// Set the active locale from a preference, notifying listeners when the
/// resolved locale actually changed. Returns the resolved locale.
pub fn set_preference(pref: LanguagePref) -> Locale {
    let resolved = resolve(pref);
    set_locale_with(resolved, LocaleChange::User)
}

/// Set the active locale to an explicit value, notifying listeners when the
/// resolved locale actually changed. Returns the resolved locale.
pub fn set_locale(next: Locale) -> Locale {
    set_locale_with(next, LocaleChange::User)
}

fn set_locale_with(next: Locale, reason: LocaleChange) -> Locale {
    let previous = locale();
    if previous != next {
        ACTIVE.store(discriminant(next), Ordering::Relaxed);
        if let Ok(guard) = listeners().lock() {
            for listener in guard.iter() {
                listener(next, reason);
            }
        }
    }
    next
}

/// Resolve and install the process locale from a preference without firing the
/// `User` change notification. Used once during startup.
pub fn init_from_preference(pref: LanguagePref) -> Locale {
    set_locale_with(resolve(pref), LocaleChange::Initial)
}

/// Resolve and install the process locale from a preference, falling back to a
/// caller-supplied locale when detection finds nothing (for example the server,
/// which should default to the configured locale rather than English).
pub fn init_from_preference_or(pref: LanguagePref, fallback: Locale) -> Locale {
    match pref {
        LanguagePref::Locale(locale) => locale,
        LanguagePref::Auto => detect::detect_locale_full().unwrap_or(fallback),
    }
}

/// Remember every missing key so a test (or a developer build) can report them
/// once instead of per-render.
fn missing_keys() -> &'static Mutex<Vec<String>> {
    static MISSING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    MISSING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Keys that were requested but are absent from every catalog.
pub fn reported_missing_keys() -> Vec<String> {
    missing_keys().lock().map(|k| k.clone()).unwrap_or_default()
}

/// Clear the missing-key log, so a test can measure one code path in isolation.
pub fn clear_missing_keys() {
    if let Ok(mut guard) = missing_keys().lock() {
        guard.clear();
    }
}

/// Look up `key` for an explicit locale without touching the active one.
///
/// HTTP request handlers use this so two concurrent requests in different
/// languages cannot change each other's rendering.
pub fn t_in(locale: Locale, key: &str) -> String {
    match catalog::lookup(locale, key) {
        Some(value) => value.to_string(),
        None => {
            record_missing(key);
            key.to_string()
        }
    }
}

/// Look up `key` for an explicit locale and render it with `args`.
pub fn t_args_in(locale: Locale, key: &str, args: &[ArgValue]) -> String {
    let Some(pattern) = catalog::lookup(locale, key) else {
        record_missing(key);
        return key.to_string();
    };
    message::render(locale, pattern, args)
}

/// Render `pattern` for an explicit locale.
pub fn render_pattern_in(locale: Locale, pattern: &str, args: &[ArgValue]) -> String {
    message::render(locale, pattern, args)
}

/// Look up `key` and return it verbatim. Unknown keys return the key itself,
/// which is the whole point of the naming convention: a visible key in the UI
/// is an obvious bug report, whereas an empty string is not.
pub fn t(key: &str) -> String {
    let locale = locale();
    match catalog::lookup(locale, key) {
        Some(value) => value.to_string(),
        None => {
            record_missing(key);
            key.to_string()
        }
    }
}

/// Look up `key` without falling back to English. Reports a missing key when the
/// active locale has no entry, which is how the desktop shows a
/// "translation missing" marker in developer builds.
pub fn t_strict(key: &str) -> Option<String> {
    let locale = locale();
    match catalog::lookup_strict(locale, key) {
        Some(value) => Some(value.to_string()),
        None => {
            record_missing(key);
            None
        }
    }
}

fn record_missing(key: &str) {
    if let Ok(mut guard) = missing_keys().lock() {
        if !guard.iter().any(|k| k == key) {
            guard.push(key.to_string());
        }
    }
}

/// Look up `key` and render it with `args`.
pub fn t_args(key: &str, args: &[ArgValue]) -> String {
    let locale = locale();
    let Some(pattern) = catalog::lookup(locale, key) else {
        record_missing(key);
        return key.to_string();
    };
    message::render(locale, pattern, args)
}

/// Look up `key` and render it, reporting whether a fallback was needed.
pub fn t_args_checked(key: &str, args: &[ArgValue]) -> message::Rendered {
    let locale = locale();
    let Some(pattern) = catalog::lookup(locale, key) else {
        record_missing(key);
        return message::Rendered {
            text: key.to_string(),
            fell_back: true,
        };
    };
    message::render_checked(locale, pattern, args)
}

/// Render `pattern` (not a key) in the active locale. For text assembled in
/// Rust that is not yet worth a catalog entry.
pub fn render_pattern(pattern: &str, args: &[ArgValue]) -> String {
    message::render(locale(), pattern, args)
}

// ---------------------------------------------------------------------------
// Locale-aware value formatting, bound to the active locale.
//
// These exist so call sites read `count(turns)` rather than
// `format_count(locale(), turns)`: the locale is always the active one and
// forgetting it was the entire class of bug the i18n layer is here to remove.
// ---------------------------------------------------------------------------

/// An integer with grouping separators for the active locale.
pub fn count(value: u64) -> String {
    format::format_count(locale(), value)
}

/// A signed integer with grouping separators for the active locale.
pub fn int(value: i64) -> String {
    format::format_integer(locale(), value)
}

/// A float rendered with a locale-appropriate number style.
pub fn number(value: f64, style: format::NumberStyle) -> String {
    format::format_number(locale(), value, style)
}

/// A `0.0..=1.0` ratio as a localized percentage.
pub fn percent(ratio: f64) -> String {
    format::format_percent(locale(), ratio)
}

/// A byte count as a localized, binary-prefixed size.
pub fn bytes(value: u64) -> String {
    format::format_bytes(locale(), value)
}

/// A duration in seconds as a compact, localized string.
pub fn duration(secs: u64) -> String {
    format::format_duration(locale(), secs)
}

/// A timestamp as a localized date.
pub fn date(at: &chrono::DateTime<chrono::Utc>, style: format::DateStyle) -> String {
    format::format_date(locale(), at, style)
}

/// A timestamp as a localized time.
pub fn time(at: &chrono::DateTime<chrono::Utc>, style: format::TimeStyle) -> String {
    format::format_time(locale(), at, style)
}

/// A timestamp as a localized date and time.
pub fn datetime(
    at: &chrono::DateTime<chrono::Utc>,
    date_style: format::DateStyle,
    time_style: format::TimeStyle,
) -> String {
    format::format_datetime(locale(), at, date_style, time_style)
}

/// A duration split into whole units.
pub fn duration_parts(secs: u64) -> format::DurationParts {
    format::DurationParts::from_secs(secs)
}

/// The active locale's compact rendering of a one-second duration, for callers
/// that need the unit abbreviation on its own (a column header, a tooltip).
pub fn duration_unit_sample() -> String {
    format::format_duration(locale(), 2)
}

/// True when the active locale needs wide-glyph handling in layout.
pub fn is_cjk() -> bool {
    locale().is_cjk()
}

/// The `Content-Language` value for the active locale.
pub fn content_language() -> &'static str {
    locale().html_lang()
}

/// Shorthand for building an [`ArgValue`] list.
pub fn args(pairs: Vec<ArgValue>) -> Vec<ArgValue> {
    pairs
}

/// A borrowed `&str` argument.
pub fn s(value: &str) -> Arg {
    Arg::Str(value.to_string())
}

/// An owned `String` argument.
pub fn owned(value: String) -> Arg {
    Arg::Str(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{DateStyle, NumberStyle, TimeStyle};
    use std::sync::{Mutex as StdMutex, MutexGuard, OnceLock};

    /// The active locale is process-global, so locale-sensitive tests take this
    /// lock and reset to English afterwards.
    fn lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct Reset(Locale);

    impl Drop for Reset {
        fn drop(&mut self) {
            let _ = set_locale_with(self.0, LocaleChange::Initial);
        }
    }

    #[test]
    fn lookup_follows_the_active_locale() {
        let _guard = lock();
        let _reset = Reset(locale());
        set_locale(Locale::En);
        let english = t("common.cancel");
        set_locale(Locale::ZhHans);
        let chinese = t("common.cancel");
        assert_eq!(english, "Cancel");
        assert_eq!(chinese, "取消");
    }

    #[test]
    fn unknown_keys_return_the_key_and_are_reported_once() {
        let _guard = lock();
        let _reset = Reset(locale());
        clear_missing_keys();
        assert_eq!(t("no.such.key"), "no.such.key");
        assert_eq!(t("no.such.key"), "no.such.key");
        let missing = reported_missing_keys();
        assert_eq!(
            missing.iter().filter(|k| *k == "no.such.key").count(),
            1,
            "a missing key must be recorded once, not per render"
        );
        clear_missing_keys();
    }

    #[test]
    fn arguments_are_interpolated() {
        let _guard = lock();
        let _reset = Reset(locale());
        set_locale(Locale::En);
        let rendered = t_args("session.plans_count", &[ArgValue::new("count", 3i64)]);
        assert_eq!(rendered, "Plans (3)");
    }

    #[test]
    fn formatted_helpers_follow_the_active_locale() {
        let _guard = lock();
        let _reset = Reset(locale());
        set_locale(Locale::En);
        assert_eq!(count(12345), "12,345");
        assert_eq!(int(-12345), "-12,345");
        assert_eq!(percent(0.5), "50.0%");
        assert_eq!(bytes(2048), "2 KB");
        assert_eq!(duration(7500), "2h 5min");
        assert_eq!(number(1.5, NumberStyle::OneDecimal), "1.5");

        set_locale(Locale::De);
        assert_eq!(count(12345), "12.345");
        assert_eq!(percent(0.5), "50,0%");
        assert_eq!(duration(7500), "2 Std. 5 Min.");
        assert_eq!(content_language(), "de-DE");
        assert!(!is_cjk());

        set_locale(Locale::Ja);
        assert_eq!(count(12345), "12,345");
        assert_eq!(duration(7500), "2時間5分");
        assert!(is_cjk());
    }

    #[test]
    fn dates_follow_the_active_locale() {
        let _guard = lock();
        let _reset = Reset(locale());
        use chrono::TimeZone;
        let at = chrono::Utc.with_ymd_and_hms(2026, 3, 9, 14, 5, 0).unwrap();
        set_locale(Locale::En);
        assert_eq!(date(&at, DateStyle::Long), "March 9, 2026");
        assert_eq!(time(&at, TimeStyle::Short), "14:05");
        assert_eq!(
            datetime(&at, DateStyle::Medium, TimeStyle::Short),
            "Mar 9, 2026 14:05"
        );
        set_locale(Locale::ZhHant);
        assert_eq!(date(&at, DateStyle::Long), "2026年3月9日");
    }

    #[test]
    fn changing_locale_notifies_listeners_exactly_once() {
        let _guard = lock();
        let _reset = Reset(locale());
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let counter_clone = counter.clone();
        let seen_clone = seen.clone();
        on_locale_change(move |locale, reason| {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Ok(mut guard) = seen_clone.lock() {
                guard.push((locale, reason));
            }
        });
        set_locale(Locale::Ru);
        set_locale(Locale::Ru);
        set_locale(Locale::Ko);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
        let guard = seen.lock().unwrap();
        let recorded: Vec<(Locale, LocaleChange)> = guard.clone();
        drop(guard);
        assert_eq!(
            recorded,
            vec![
                (Locale::Ru, LocaleChange::User),
                (Locale::Ko, LocaleChange::User)
            ]
        );
    }

    #[test]
    fn startup_resolution_does_not_announce_a_user_change() {
        let _guard = lock();
        let _reset = Reset(locale());
        let reason = std::sync::Arc::new(Mutex::new(None));
        let reason_clone = reason.clone();
        on_locale_change(move |_locale, change| {
            if let Ok(mut guard) = reason_clone.lock() {
                *guard = Some(change);
            }
        });
        init_from_preference(LanguagePref::Locale(Locale::Fr));
        assert_eq!(locale(), Locale::Fr);
        assert_eq!(*reason.lock().unwrap(), Some(LocaleChange::Initial));
    }

    #[test]
    fn explicit_locale_rendering_does_not_change_the_active_locale() {
        let _guard = lock();
        let _reset = Reset(locale());
        set_locale(Locale::En);
        assert_eq!(t_in(Locale::ZhHans, "common.cancel"), "取消");
        assert_eq!(
            locale(),
            Locale::En,
            "explicit rendering must be side-effect free"
        );
        assert_eq!(
            t_args_in(
                Locale::De,
                "session.plans_count",
                &[ArgValue::new("count", 3i64)]
            ),
            "Pläne (3)"
        );
        assert_eq!(locale(), Locale::En);
    }

    #[test]
    fn discriminant_round_trips_for_every_locale() {
        for locale in Locale::ALL {
            assert_eq!(from_discriminant(discriminant(locale)), locale);
        }
    }

    #[test]
    fn duration_helpers_are_usable() {
        let _guard = lock();
        let _reset = Reset(locale());
        set_locale(Locale::En);
        assert_eq!(duration_unit_sample(), "2s");
    }
}
