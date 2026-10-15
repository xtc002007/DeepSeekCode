//! Internationalization for CodePlusPlus: locale detection, message catalogs,
//! and locale-aware value formatting.
//!
//! # Shape
//!
//! - [`locale`] — the ten shipped locales, [`locale::LanguagePref`], and BCP-47
//!   parsing tolerant of POSIX locale strings and Windows display names.
//! - [`detect`] — `CODE_PLUS_PLUS_LANG` → POSIX chain → Windows UI language.
//! - [`catalog`] — embedded JSON catalogs with English fallback.
//! - [`message`] — an ICU MessageFormat subset with CLDR plural selection.
//! - [`plural`] — CLDR cardinal plural rules.
//! - [`format`] — locale-aware numbers, percentages, byte sizes, durations,
//!   dates and times.
//! - [`runtime`] — the active locale and the `t`/`tr!` entry points.
//!
//! # Using it
//!
//! ```no_run
//! use code_plus_plus_i18n::tr;
//!
//! // A fixed string.
//! let label = code_plus_plus_i18n::t("common.cancel");
//!
//! // A message with arguments.
//! let plans = tr!("session.plans_count", "count" => 3usize);
//!
//! // Values formatted for the active locale.
//! let tokens = code_plus_plus_i18n::count(12345);
//! ```
//!
//! # Rules for contributors
//!
//! 1. Every user-visible string lives in all ten catalogs. English defines the
//!    key set; [`catalog`] and the crate tests fail the build on a missing key.
//! 2. Placeholders must survive translation: the parity test compares the
//!    placeholder sets of every locale against English.
//! 3. Never concatenate translated fragments. Use a `{placeholder}` or a
//!    `plural` arm so word order stays under the translator's control.
//! 4. Format numbers, dates, durations and byte counts through [`runtime`]'s
//!    helpers, never through `format!`. German and Russian number separators
//!    differ from English, and unit abbreviations are not English.
//! 5. System prompts and agent-facing tool descriptions stay English: they are
//!    read by models, not people, and a localized prompt would fork the
//!    provider prefix cache on every language change.

#![deny(missing_docs)]

pub mod catalog;
pub mod detect;
pub mod format;
pub mod locale;
pub mod message;
pub mod plural;
pub mod runtime;

pub use locale::{Direction, LanguagePref, Locale};
pub use message::{Arg, ArgValue};
pub use runtime::{
    bytes, content_language, count, date, datetime, duration, duration_parts, init_from_preference,
    init_from_preference_or, int, is_cjk, locale, number, on_locale_change, percent,
    render_pattern, render_pattern_in, resolve, set_locale, set_preference, t, t_args,
    t_args_checked, t_args_in, t_in, t_strict, time, LocaleChange,
};

/// Translate `key` with no arguments.
///
/// Prefer this over [`t`] in expression position: it reads as an intent, and it
/// is the form the catalog-parity tooling scans for.
///
/// Argument names must be string literals. That is deliberate — the catalog and
/// the call site have to agree on the placeholder name at build time, which is
/// what lets a missing or misspelled placeholder be a compile error rather than
/// a `{count}` appearing in the UI.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::runtime::t($key)
    };
    ($key:expr, $($name:literal => $value:expr),+ $(,)?) => {
        $crate::runtime::t_args(
            $key,
            &[$($crate::message::ArgValue {
                name: $name,
                value: $crate::message::Arg::from($value),
            }),+],
        )
    };
}
