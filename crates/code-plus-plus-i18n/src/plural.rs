//! CLDR cardinal plural rules for the shipped locales.
//!
//! The rules are transcribed from the CLDR `plurals.xml` cardinal set. They
//! operate on the same operands ICU names (`n` absolute value, `i` integer
//! digits, `v` visible fraction digits, `w` visible fraction digits without
//! trailing zeros, `f` visible fraction digits as an integer, `t` likewise
//! without trailing zeros), so a locale can be added by transcribing one
//! `<pluralRules>` element and nothing else changes.

use crate::locale::Locale;

/// A CLDR plural category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluralCategory {
    /// CLDR `zero`.
    Zero,
    /// CLDR `one`.
    One,
    /// CLDR `two` (dual form; no shipped locale uses it).
    Two,
    /// CLDR `few` (the Slavic paucal form).
    Few,
    /// CLDR `many` (the Slavic form, and the Romance compact form).
    Many,
    /// CLDR `other`: the mandatory fallback arm.
    Other,
}

impl PluralCategory {
    /// Select the matching arm name in a message. `Other` is the mandatory
    /// fallback arm.
    pub const fn as_str(self) -> &'static str {
        match self {
            PluralCategory::Zero => "zero",
            PluralCategory::One => "one",
            PluralCategory::Two => "two",
            PluralCategory::Few => "few",
            PluralCategory::Many => "many",
            PluralCategory::Other => "other",
        }
    }
}

/// The operands CLDR plural rules are written against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Operands {
    /// Absolute value of the source number.
    pub n: f64,
    /// Integer digits of `n`.
    pub i: i64,
    /// Number of visible fraction digits, with trailing zeros.
    pub v: u32,
    /// Number of visible fraction digits, without trailing zeros.
    pub w: u32,
    /// Visible fraction digits as an integer, with trailing zeros.
    pub f: u64,
    /// Visible fraction digits as an integer, without trailing zeros.
    pub t: u64,
}

impl Operands {
    /// Derive operands from a plain integer. No visible fraction digits.
    pub fn from_int(value: i64) -> Self {
        Self {
            n: value.unsigned_abs() as f64,
            // CLDR `i` is the absolute value of the integer digits.
            i: value.unsigned_abs() as i64,
            v: 0,
            w: 0,
            f: 0,
            t: 0,
        }
    }

    /// Derive operands from an unsigned count.
    pub fn from_u64(value: u64) -> Self {
        Self::from_int(value.min(i64::MAX as u64) as i64)
    }

    /// Derive operands from a float, exposing up to `max_fraction_digits`
    /// fraction digits so `v` reflects what the user will actually see.
    pub fn from_float(value: f64, max_fraction_digits: u32) -> Self {
        let abs = value.abs();
        let scale = 10f64.powi(max_fraction_digits as i32);
        // Round half away from zero at the display precision, matching what
        // `format.rs` will emit.
        let scaled = (abs * scale).round() as u64;
        let divisor = 10u64.pow(max_fraction_digits);
        let (i, frac) = if max_fraction_digits == 0 {
            (scaled, 0)
        } else {
            (scaled / divisor, scaled % divisor)
        };
        let i = i.min(i64::MAX as u64) as i64;
        let frac_str = format!("{frac:0width$}", width = max_fraction_digits as usize);
        let trimmed = frac_str.trim_end_matches('0');
        // `v` counts the visible fraction digits *including* trailing zeros,
        // so 2.50 shows two of them and 2.5 shows one.
        let v = if frac == 0 { 0 } else { max_fraction_digits };
        let w = if frac == 0 { 0 } else { trimmed.len() as u32 };
        let f = frac;
        let t = trimmed.parse::<u64>().unwrap_or(0);
        Self {
            n: abs,
            i,
            v,
            w,
            f,
            t,
        }
    }
}

impl Operands {
    fn is_integer(&self) -> bool {
        self.v == 0
    }

    /// `e` in CLDR terms: the compact-decimal exponent. Always 0 here because
    /// no shipped locale ships a compact-number format.
    fn exponent_is_zero(&self) -> bool {
        true
    }
}

/// Select the plural category for `operands` under `locale`'s cardinal rules.
pub fn cardinal(locale: Locale, operands: &Operands) -> PluralCategory {
    match locale {
        // No plural distinction: Japanese, Korean and both Chinese scripts
        // have a single `other` form.
        Locale::ZhHans | Locale::ZhHant | Locale::Ja | Locale::Ko => PluralCategory::Other,

        // English, German and the rest of the Germanic/Slavic-adjacent set we
        // ship: `one` only for exactly 1.
        Locale::En | Locale::De => {
            if operands.is_integer() && operands.i == 1 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }

        // Spanish: `one` for 1 regardless of visible fraction digits; plus the
        // compact `many` for round millions.
        Locale::Es => {
            if operands.n == 1.0 {
                PluralCategory::One
            } else if is_round_million(operands) {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }

        // French and Brazilian Portuguese: `one` covers 0 and 1 (French
        // "0 fichier" is correct), same compact `many`.
        Locale::Fr | Locale::PtBr => {
            if (0..=1).contains(&operands.i) && operands.is_integer() {
                PluralCategory::One
            } else if is_round_million(operands) {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }

        // Russian: the three-form Slavic pattern driven by the last two digits.
        Locale::Ru => {
            if !operands.is_integer() {
                return PluralCategory::Other;
            }
            let i = operands.i;
            let mod10 = i % 10;
            let mod100 = i % 100;
            if mod10 == 1 && !(11..=14).contains(&mod100) {
                PluralCategory::One
            } else if (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                PluralCategory::Few
            } else {
                PluralCategory::Many
            }
        }
    }
}

/// CLDR `many` for the Romance locales: a whole number of millions.
fn is_round_million(operands: &Operands) -> bool {
    operands.exponent_is_zero()
        && operands.is_integer()
        && operands.i != 0
        && operands.i % 1_000_000 == 0
}

/// Human-readable dump of the operands, for test failure messages.
#[cfg(test)]
pub(crate) fn debug_operands(operands: &Operands) -> String {
    format!(
        "n={} i={} v={} w={} f={} t={}",
        operands.n, operands.i, operands.v, operands.w, operands.f, operands.t
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(locale: Locale, value: i64) -> PluralCategory {
        cardinal(locale, &Operands::from_int(value))
    }

    #[test]
    fn english_and_german_have_two_forms() {
        for locale in [Locale::En, Locale::De] {
            assert_eq!(cat(locale, 0), PluralCategory::Other, "{locale:?} 0");
            assert_eq!(cat(locale, 1), PluralCategory::One, "{locale:?} 1");
            assert_eq!(cat(locale, 2), PluralCategory::Other, "{locale:?} 2");
            assert_eq!(cat(locale, 11), PluralCategory::Other, "{locale:?} 11");
            assert_eq!(cat(locale, -1), PluralCategory::One, "{locale:?} -1");
        }
    }

    #[test]
    fn cjk_locales_have_a_single_form() {
        for locale in [Locale::ZhHans, Locale::ZhHant, Locale::Ja, Locale::Ko] {
            for value in [0, 1, 2, 5, 11, 100] {
                assert_eq!(
                    cat(locale, value),
                    PluralCategory::Other,
                    "{locale:?} {value}"
                );
            }
        }
    }

    #[test]
    fn russian_has_one_few_many() {
        // one: 1, 21, 31, 101, 1001 …
        for value in [1, 21, 31, 101, 1001, -21] {
            assert_eq!(cat(Locale::Ru, value), PluralCategory::One, "{value}");
        }
        // few: 2..4, 22..24 …
        for value in [2, 3, 4, 22, 23, 24, 102] {
            assert_eq!(cat(Locale::Ru, value), PluralCategory::Few, "{value}");
        }
        // many: 0, 5..20, 25..30, 100 …
        for value in [0, 5, 11, 12, 14, 19, 25, 100, 111] {
            assert_eq!(cat(Locale::Ru, value), PluralCategory::Many, "{value}");
        }
    }

    #[test]
    fn spanish_treats_one_as_singular_even_with_decimals() {
        assert_eq!(cat(Locale::Es, 1), PluralCategory::One);
        assert_eq!(
            cardinal(Locale::Es, &Operands::from_float(1.0, 2)),
            PluralCategory::One
        );
        assert_eq!(cat(Locale::Es, 0), PluralCategory::Other);
        assert_eq!(cat(Locale::Es, 2), PluralCategory::Other);
        // compact `many`: whole millions
        assert_eq!(cat(Locale::Es, 2_000_000), PluralCategory::Many);
    }

    #[test]
    fn french_and_portuguese_include_zero_in_one() {
        for locale in [Locale::Fr, Locale::PtBr] {
            assert_eq!(cat(locale, 0), PluralCategory::One, "{locale:?} 0");
            assert_eq!(cat(locale, 1), PluralCategory::One, "{locale:?} 1");
            assert_eq!(cat(locale, 2), PluralCategory::Other, "{locale:?} 2");
            assert_eq!(
                cat(locale, 1_000_000),
                PluralCategory::Many,
                "{locale:?} 1e6"
            );
        }
    }

    #[test]
    fn fractional_values_fall_outside_the_integer_only_rules() {
        let ops = Operands::from_float(1.5, 2);
        assert_eq!(ops.v, 2, "{}", debug_operands(&ops));
        assert_eq!(ops.f, 50);
        assert_eq!(cat_from_ops(Locale::En, &ops), PluralCategory::Other);
        assert_eq!(cat_from_ops(Locale::Ru, &ops), PluralCategory::Other);
        // French `one` requires `v = 0`.
        assert_eq!(cat_from_ops(Locale::Fr, &ops), PluralCategory::Other);
    }

    fn cat_from_ops(locale: Locale, ops: &Operands) -> PluralCategory {
        cardinal(locale, ops)
    }

    #[test]
    fn float_operands_track_visible_fraction_digits() {
        let ops = Operands::from_float(2.0, 2);
        assert_eq!(ops.i, 2);
        assert_eq!(ops.v, 0, "{}", debug_operands(&ops));
        assert_eq!(ops.w, 0);

        // Two digits of precision on 2.50 shows both digits, with `w` and `t`
        // reflecting the value with the trailing zero stripped.
        let ops = Operands::from_float(2.50, 2);
        assert_eq!(ops.i, 2);
        assert_eq!(ops.v, 2, "{}", debug_operands(&ops));
        assert_eq!(ops.f, 50);
        assert_eq!(ops.t, 5, "{}", debug_operands(&ops));
        assert_eq!(ops.w, 1);

        // CLDR `i` is unsigned.
        let ops = Operands::from_float(-3.25, 2);
        assert_eq!(ops.n, 3.25);
        assert_eq!(ops.i, 3);
        assert_eq!(ops.f, 25);
    }

    #[test]
    fn every_category_name_is_stable() {
        assert_eq!(PluralCategory::Zero.as_str(), "zero");
        assert_eq!(PluralCategory::One.as_str(), "one");
        assert_eq!(PluralCategory::Two.as_str(), "two");
        assert_eq!(PluralCategory::Few.as_str(), "few");
        assert_eq!(PluralCategory::Many.as_str(), "many");
        assert_eq!(PluralCategory::Other.as_str(), "other");
    }
}
