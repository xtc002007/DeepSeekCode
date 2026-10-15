//! Locale-aware formatting for numbers, percentages, byte sizes, durations,
//! dates and times.
//!
//! Every table here is the CLDR data for the shipped locales, transcribed into
//! Rust so the catalog files stay pure user-visible copy. Nothing in this module
//! is English-specific: grouping separators, decimal marks, unit abbreviations,
//! month names and date field order all change with the locale.

use crate::locale::Locale;
use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

/// Decimal and grouping marks for a locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberSymbols {
    /// The decimal separator character.
    pub decimal: char,
    /// The digit-group separator. May be a multi-character string (a
    /// no-break space, for example) and must never allow a line break.
    pub group: &'static str,
    /// Group widths, applied right to left: the first is the rightmost group.
    pub group_sizes: &'static [u8],
}

const LATN_STANDARD: NumberSymbols = NumberSymbols {
    decimal: '.',
    group: ",",
    group_sizes: &[3],
};
const LATN_COMMA: NumberSymbols = NumberSymbols {
    decimal: ',',
    group: ".",
    group_sizes: &[3],
};
const SPACE_GROUP: NumberSymbols = NumberSymbols {
    // U+00A0 NO-BREAK SPACE: French and Russian convention, and unlike an ASCII
    // space it must not wrap mid-number.
    decimal: ',',
    group: "\u{00a0}",
    group_sizes: &[3],
};

/// CLDR number symbols for a locale.
pub const fn number_symbols(locale: Locale) -> NumberSymbols {
    match locale {
        // en, ja, ko, zh: comma group, dot decimal.
        Locale::En | Locale::Ja | Locale::Ko | Locale::ZhHans | Locale::ZhHant => LATN_STANDARD,
        // de, es, pt-BR: dot group, comma decimal.
        Locale::De | Locale::Es | Locale::PtBr => LATN_COMMA,
        // fr, ru: non-breaking-space group, comma decimal.
        Locale::Fr | Locale::Ru => SPACE_GROUP,
    }
}

/// How many fraction digits a `number` style shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberStyle {
    /// Locale default: up to 3 fraction digits, trailing zeros trimmed.
    Default,
    /// No fraction digits, grouped.
    Integer,
    /// Fixed single fraction digit (`hit rate`).
    OneDecimal,
    /// Exactly two fraction digits (`cost`).
    TwoDecimals,
    /// Multiply by 100 and append the locale percent sign.
    Percent,
    /// Compact binary byte size with the locale's unit abbreviation.
    Bytes,
    /// Grouped, no fraction digits, no scaling — for identifiers and counts
    /// that must stay exact.
    Grouped,
}

/// Percent sign per locale. Russian uses the Cyrillic `П` abbreviation and
/// Arabic-style layouts are out of scope, so Russian gets `проц.`-free short
/// form: CLDR `ru` uses `проц.` for long and `%` for short/narrow. We use `%`,
/// which is what every Russian UI in practice renders.
const PERCENT_SUFFIX: char = '%';

/// Render an integer with grouping separators for `locale`.
pub fn format_integer(locale: Locale, value: i64) -> String {
    let symbols = number_symbols(locale);
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let grouped = group_digits(&digits, symbols);
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Render an integer without grouping separators.
///
/// Date and time fields never carry grouping separators in CLDR — a year reads
/// `2026`, never `2,026` — so every calendar component uses this.
pub fn format_integer_plain(locale: Locale, value: i64) -> String {
    let _ = locale;
    value.to_string()
}

/// Render an integer zero-padded to at least `width` digits, ungrouped.
pub fn format_integer_padded(locale: Locale, value: i64, width: usize) -> String {
    let _ = locale;
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    if digits.len() >= width {
        return if negative {
            format!("-{digits}")
        } else {
            digits
        };
    }
    let padded = format!("{}{digits}", "0".repeat(width - digits.len()));
    if negative {
        format!("-{padded}")
    } else {
        padded
    }
}

/// Render an unsigned count with grouping separators for `locale`.
pub fn format_count(locale: Locale, value: u64) -> String {
    let symbols = number_symbols(locale);
    group_digits(&value.to_string(), symbols)
}

fn group_digits(digits: &str, symbols: NumberSymbols) -> String {
    if symbols.group.is_empty() {
        return digits.to_string();
    }
    let primary = *symbols.group_sizes.first().unwrap_or(&3) as usize;
    let default_secondary = primary as u8;
    let secondary = *symbols.group_sizes.get(1).unwrap_or(&default_secondary) as usize;
    if digits.is_empty() {
        return String::new();
    }
    let len = digits.len();
    // Groups are laid out from the right, so the leading partial group is
    // whatever is left over after the rightmost multiples of `primary`.
    let first = len % primary;
    let mut out = String::with_capacity(len + len / primary + 2);
    let mut index = 0usize;
    if first > 0 {
        out.push_str(&digits[..first]);
        index = first;
    }
    let mut remaining = len - index;
    let mut use_primary = true;
    while remaining > 0 {
        if !out.is_empty() {
            out.push_str(symbols.group);
        }
        let size = if use_primary { primary } else { secondary };
        let take = size.min(remaining);
        out.push_str(&digits[index..index + take]);
        index += take;
        remaining -= take;
        use_primary = false;
    }
    out
}

/// Format a float with a `NumberStyle`.
pub fn format_number(locale: Locale, value: f64, style: NumberStyle) -> String {
    if !value.is_finite() {
        // NaN / ±Inf have no locale spelling worth inventing; render a stable
        // placeholder so the UI never prints `NaN` or `inf`.
        return "\u{2014}".to_string();
    }
    match style {
        NumberStyle::Bytes => format_bytes(locale, value.max(0.0) as u64),
        NumberStyle::Percent => format_percent(locale, value),
        NumberStyle::Integer | NumberStyle::Grouped => format_integer(locale, value.round() as i64),
        NumberStyle::Default => format_decimal(locale, value, 3, true),
        NumberStyle::OneDecimal => format_decimal(locale, value, 1, true),
        NumberStyle::TwoDecimals => format_decimal(locale, value, 2, true),
    }
}

fn format_decimal(locale: Locale, value: f64, digits: u32, trim: bool) -> String {
    let symbols = number_symbols(locale);
    let negative = value < 0.0;
    let abs = value.abs();
    let scale = 10f64.powi(digits as i32);
    let scaled = (abs * scale).round() as u64;
    let int_part = if digits == 0 {
        scaled
    } else {
        scaled / 10u64.pow(digits)
    };
    let mut frac = if digits == 0 {
        String::new()
    } else {
        format!(
            "{:0width$}",
            scaled % 10u64.pow(digits),
            width = digits as usize
        )
    };
    if trim {
        while frac.ends_with('0') {
            frac.pop();
        }
    }
    let mut out = group_digits(&int_part.to_string(), symbols);
    if !frac.is_empty() {
        out.push(symbols.decimal);
        out.push_str(&frac);
    }
    if negative && (int_part != 0 || frac != "0") {
        format!("-{out}")
    } else {
        out
    }
}

/// Format a ratio in `0.0..=1.0` as a localized percentage.
pub fn format_percent(locale: Locale, ratio: f64) -> String {
    let symbols = number_symbols(locale);
    let value = ratio * 100.0;
    let scaled = (value.abs() * 10.0).round() as u64;
    let int_part = scaled / 10;
    let one_dec = scaled % 10;
    let mut out = String::new();
    if value < 0.0 && (int_part != 0 || one_dec != 0) {
        out.push('-');
    }
    out.push_str(&group_digits(&int_part.to_string(), symbols));
    out.push(symbols.decimal);
    out.push(char::from(b'0' + one_dec as u8));
    out.push(PERCENT_SUFFIX);
    out
}

/// Byte-size unit abbreviations per locale. Binary multiples (1024-based) with
/// the labels users actually expect (`KB`, not `KiB`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ByteUnits {
    /// Bytes (`B`).
    pub one: &'static str,
    /// Kibibytes, labelled the way users expect (`KB`).
    pub kilo: &'static str,
    /// Mebibytes (`MB`).
    pub mega: &'static str,
    /// Gibibytes (`GB`).
    pub giga: &'static str,
    /// Tebibytes (`TB`).
    pub tera: &'static str,
    /// Pebibytes (`PB`).
    pub peta: &'static str,
}

const BYTE_UNITS_LATN: ByteUnits = ByteUnits {
    one: "B",
    kilo: "KB",
    mega: "MB",
    giga: "GB",
    tera: "TB",
    peta: "PB",
};
const BYTE_UNITS_RU: ByteUnits = ByteUnits {
    one: "Б",
    kilo: "КБ",
    mega: "МБ",
    giga: "ГБ",
    tera: "ТБ",
    peta: "ПБ",
};
const BYTE_UNITS_JA: ByteUnits = ByteUnits {
    one: "B",
    kilo: "KB",
    mega: "MB",
    giga: "GB",
    tera: "TB",
    peta: "PB",
};
const BYTE_UNITS_KO: ByteUnits = ByteUnits {
    one: "B",
    kilo: "KB",
    mega: "MB",
    giga: "GB",
    tera: "TB",
    peta: "PB",
};

const fn byte_units(locale: Locale) -> ByteUnits {
    match locale {
        Locale::En
        | Locale::De
        | Locale::Es
        | Locale::Fr
        | Locale::PtBr
        | Locale::ZhHans
        | Locale::ZhHant => BYTE_UNITS_LATN,
        Locale::Ru => BYTE_UNITS_RU,
        Locale::Ja => BYTE_UNITS_JA,
        Locale::Ko => BYTE_UNITS_KO,
    }
}

/// Human byte size using 1024-based multiples, with locale-correct digits and
/// unit labels. Values below 1 KiB keep a single decimal (`1.5 KB`); larger
/// units use at most two decimals with trailing zeros trimmed.
pub fn format_bytes(locale: Locale, bytes: u64) -> String {
    let units = byte_units(locale);
    const KIB: f64 = 1024.0;
    if bytes < 1024 {
        return format!("{} {}", format_count(locale, bytes), units.one);
    }
    // Start in KB, then step up only while the value would still be at least 1
    // in the next unit. Dividing first and checking afterwards would render
    // 1024 bytes as "1 MB".
    let mut value = bytes as f64 / KIB;
    let mut unit = units.kilo;
    for next in [units.mega, units.giga, units.tera, units.peta] {
        if value < KIB {
            break;
        }
        value /= KIB;
        unit = next;
    }
    let body = format_decimal(locale, value, 2, true);
    format!("{body} {unit}")
}

/// Duration unit abbreviations, ordered second → minute → hour → day.
/// `None` entries mean the locale does not abbreviate that unit (it spells it
/// out in the `duration.unit.*` catalog keys instead).
#[derive(Debug, Clone, Copy)]
pub struct DurationUnits {
    /// Second abbreviation, or `None` to fall back to the catalog spelling.
    pub second: Option<&'static str>,
    /// Minute abbreviation.
    pub minute: Option<&'static str>,
    /// Hour abbreviation.
    pub hour: Option<&'static str>,
    /// Day abbreviation.
    pub day: Option<&'static str>,
}

const fn du(
    second: Option<&'static str>,
    minute: Option<&'static str>,
    hour: Option<&'static str>,
    day: Option<&'static str>,
) -> DurationUnits {
    DurationUnits {
        second,
        minute,
        hour,
        day,
    }
}

/// Compact duration abbreviations per locale. CJK and Russian conventions put
/// the unit after the number with no space; Latin-script locales use `h`/`min`
/// with a space (US/UK `h`/`m`).
const DURATION_UNITS: fn(Locale) -> DurationUnits = |locale| match locale {
    // CLDR short duration patterns. Latin-script abbreviations attach directly
    // to the number; German and French keep the space they spell with.
    Locale::En => du(Some("s"), Some("min"), Some("h"), Some("d")),
    Locale::De => du(Some("s"), Some("Min."), Some("Std."), Some("T")),
    Locale::Es => du(Some("s"), Some("min"), Some("h"), Some("d")),
    Locale::Fr => du(Some("s"), Some("min"), Some("h"), Some("j")),
    Locale::PtBr => du(Some("s"), Some("min"), Some("h"), Some("d")),
    Locale::Ru => du(Some("с"), Some("мин"), Some("ч"), Some("д")),
    Locale::ZhHans => du(Some("秒"), Some("分钟"), Some("小时"), Some("天")),
    Locale::ZhHant => du(Some("秒"), Some("分鐘"), Some("小時"), Some("天")),
    Locale::Ja => du(Some("秒"), Some("分"), Some("時間"), Some("日")),
    Locale::Ko => du(Some("초"), Some("분"), Some("시간"), Some("일")),
};

/// Separator *between* duration components (not between a number and its unit).
/// CJK writes `2時間5分` with no gap; every other shipped locale separates the
/// components with a space so `1h 30m` stays readable.
const fn component_separator(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans | Locale::ZhHant | Locale::Ja | Locale::Ko => "",
        Locale::En | Locale::Es | Locale::Fr | Locale::De | Locale::PtBr | Locale::Ru => " ",
    }
}

/// Locale-appropriate glue between a number and its unit abbreviation.
const fn unit_separator(locale: Locale) -> &'static str {
    match locale {
        // German, French and Russian spell a space before the unit; the Latin
        // CLDR patterns and the CJK scripts attach it directly.
        Locale::De | Locale::Fr | Locale::Ru => " ",
        Locale::En
        | Locale::Es
        | Locale::PtBr
        | Locale::ZhHans
        | Locale::ZhHant
        | Locale::Ja
        | Locale::Ko => "",
    }
}
/// example a tooltip that spells the full duration out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationParts {
    /// Whole days.
    pub days: u64,
    /// Hours within the day.
    pub hours: u64,
    /// Minutes within the hour.
    pub minutes: u64,
    /// Seconds within the minute.
    pub seconds: u64,
}

impl DurationParts {
    /// Split a second count into whole day/hour/minute/second components.
    pub fn from_secs(secs: u64) -> Self {
        Self {
            days: secs / 86_400,
            hours: (secs % 86_400) / 3_600,
            minutes: (secs % 3_600) / 60,
            seconds: secs % 60,
        }
    }

    /// Total seconds back, for round-tripping through storage.
    pub fn total_secs(self) -> u64 {
        self.days * 86_400 + self.hours * 3_600 + self.minutes * 60 + self.seconds
    }

    /// True when the span is zero seconds.
    pub fn is_zero(self) -> bool {
        self.total_secs() == 0
    }
}

/// Compact duration in `locale`, dropping zero components but never returning
/// an empty string. `0` renders as the locale's sub-second form
/// (`<1s` / `<1 秒` / `<1 с`), matching the pre-i18n behaviour.
pub fn format_duration(locale: Locale, secs: u64) -> String {
    let units = DURATION_UNITS(locale);
    let sep = unit_separator(locale);
    let joiner = component_separator(locale);
    if secs == 0 {
        return sub_second(locale, units);
    }
    let parts = DurationParts::from_secs(secs);
    let mut out: Vec<String> = Vec::with_capacity(4);
    let mut push = |value: u64, unit: Option<&'static str>| {
        if value == 0 {
            return;
        }
        let number = format_count(locale, value);
        match unit {
            Some(unit) => out.push(format!("{number}{sep}{unit}")),
            None => out.push(number),
        }
    };
    push(parts.days, units.day);
    push(parts.hours, units.hour);
    push(parts.minutes, units.minute);
    push(parts.seconds, units.second);
    if out.is_empty() {
        sub_second(locale, units)
    } else {
        out.join(joiner)
    }
}

/// The locale's rendering of "less than one second".
fn sub_second(locale: Locale, units: DurationUnits) -> String {
    match units.second {
        Some(unit) => {
            let sep = unit_separator(locale);
            format!("<1{sep}{unit}")
        }
        None => "<1s".to_string(),
    }
}

/// Date field order and naming per locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// `M/d/yy` — US numeric.
    Short,
    /// `MMM d, y` — month name, abbreviated.
    Medium,
    /// `MMMM d, y` — month name, full.
    Long,
    /// Full weekday + `Long` date.
    Full,
}

/// Time field detail per locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeStyle {
    /// `HH:mm`
    Short,
    /// `HH:mm:ss`
    Medium,
    /// `HH:mm:ss` + short zone name where the locale has one.
    Full,
}

/// Month names for a locale, January first. CLDR stand-alone forms.
pub fn month_names(locale: Locale) -> &'static [&'static str; 12] {
    match locale {
        Locale::En => &[
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ],
        Locale::ZhHans => &[
            "一月",
            "二月",
            "三月",
            "四月",
            "五月",
            "六月",
            "七月",
            "八月",
            "九月",
            "十月",
            "十一月",
            "十二月",
        ],
        Locale::ZhHant => &[
            "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
        ],
        Locale::Ja => &[
            "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
        ],
        Locale::Ko => &[
            "1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
        ],
        Locale::Es => &[
            "enero",
            "febrero",
            "marzo",
            "abril",
            "mayo",
            "junio",
            "julio",
            "agosto",
            "septiembre",
            "octubre",
            "noviembre",
            "diciembre",
        ],
        Locale::Fr => &[
            "janvier",
            "février",
            "mars",
            "avril",
            "mai",
            "juin",
            "juillet",
            "août",
            "septembre",
            "octobre",
            "novembre",
            "décembre",
        ],
        Locale::De => &[
            "Januar",
            "Februar",
            "März",
            "April",
            "Mai",
            "Juni",
            "Juli",
            "August",
            "September",
            "Oktober",
            "November",
            "Dezember",
        ],
        Locale::PtBr => &[
            "janeiro",
            "fevereiro",
            "março",
            "abril",
            "maio",
            "junho",
            "julho",
            "agosto",
            "setembro",
            "outubro",
            "novembro",
            "dezembro",
        ],
        Locale::Ru => &[
            "января",
            "февраля",
            "марта",
            "апреля",
            "мая",
            "июня",
            "июля",
            "августа",
            "сентября",
            "октября",
            "ноября",
            "декабря",
        ],
    }
}

/// Abbreviated month names for a locale, January first.
pub fn month_names_short(locale: Locale) -> &'static [&'static str; 12] {
    match locale {
        Locale::En => &[
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ],
        // CJK months are already numeric abbreviations.
        Locale::ZhHans | Locale::ZhHant | Locale::Ja => month_names(locale),
        Locale::Ko => &[
            "1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
        ],
        Locale::Es => &[
            "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
        ],
        Locale::Fr => &[
            "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
            "nov.", "décr.",
        ],
        Locale::De => &[
            "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.",
            "Dez.",
        ],
        Locale::PtBr => &[
            "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.",
            "dez.",
        ],
        Locale::Ru => &[
            "янв.",
            "февр.",
            "март",
            "апр.",
            "май",
            "июнь",
            "июль",
            "авг.",
            "сент.",
            "окт.",
            "нояб.",
            "дек.",
        ],
    }
}

/// Weekday names for a locale, Monday first (CLDR order).
pub fn weekday_names(locale: Locale) -> &'static [&'static str; 7] {
    match locale {
        Locale::En => &[
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ],
        Locale::ZhHans => &[
            "星期一",
            "星期二",
            "星期三",
            "星期四",
            "星期五",
            "星期六",
            "星期日",
        ],
        Locale::ZhHant => &[
            "星期一",
            "星期二",
            "星期三",
            "星期四",
            "星期五",
            "星期六",
            "星期日",
        ],
        Locale::Ja => &[
            "月曜日",
            "火曜日",
            "水曜日",
            "木曜日",
            "金曜日",
            "土曜日",
            "日曜日",
        ],
        Locale::Ko => &[
            "월요일",
            "화요일",
            "수요일",
            "목요일",
            "금요일",
            "토요일",
            "일요일",
        ],
        Locale::Es => &[
            "lunes",
            "martes",
            "miércoles",
            "jueves",
            "viernes",
            "sábado",
            "domingo",
        ],
        Locale::Fr => &[
            "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
        ],
        Locale::De => &[
            "Montag",
            "Dienstag",
            "Mittwoch",
            "Donnerstag",
            "Freitag",
            "Samstag",
            "Sonntag",
        ],
        Locale::PtBr => &[
            "segunda-feira",
            "terça-feira",
            "quarta-feira",
            "quinta-feira",
            "sexta-feira",
            "sábado",
            "domingo",
        ],
        Locale::Ru => &[
            "понедельник",
            "вторник",
            "среда",
            "четверг",
            "пятница",
            "суббота",
            "воскресенье",
        ],
    }
}

/// Abbreviated weekday names for a locale, Monday first.
pub fn weekday_names_short(locale: Locale) -> &'static [&'static str; 7] {
    match locale {
        Locale::En => &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        Locale::ZhHans | Locale::ZhHant => &["一", "二", "三", "四", "五", "六", "日"],
        Locale::Ja => &["月", "火", "水", "木", "金", "土", "日"],
        Locale::Ko => &["월", "화", "수", "목", "금", "토", "일"],
        Locale::Es => &["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
        Locale::Fr => &["lun", "mar", "mer", "jeu", "ven", "sam", "dim"],
        Locale::De => &["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
        Locale::PtBr => &["seg", "ter", "qua", "qui", "sex", "sáb", "dom"],
        Locale::Ru => &["пн", "вт", "ср", "чт", "пт", "сб", "вс"],
    }
}

fn weekday_index(weekday: Weekday) -> usize {
    match weekday {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    }
}

/// Format a timestamp as a local date in `locale`.
pub fn format_date(locale: Locale, at: &DateTime<Utc>, style: DateStyle) -> String {
    let y = at.year();
    let m = at.month();
    let d = at.day();
    let year = format_integer_plain(locale, i64::from(y));
    let month_num = format_integer_plain(locale, i64::from(m));
    let day = format_integer_plain(locale, i64::from(d));
    let month_short = month_names_short(locale)[m as usize - 1];
    let month_full = month_names(locale)[m as usize - 1];
    match (locale, style) {
        (_, DateStyle::Short) => match locale {
            // Month-first is the CJK convention even in numeric form.
            Locale::ZhHans | Locale::ZhHant | Locale::Ja | Locale::Ko => {
                format!("{year}/{month_num}/{day}")
            }
            // US-style for English, day-first for the rest.
            Locale::En => format!("{month_num}/{day}/{year}"),
            Locale::De => format!("{day}.{month_num}.{year}"),
            _ => format!("{day}/{month_num}/{year}"),
        },
        (_, DateStyle::Medium) => match locale {
            Locale::En => format!("{month_short} {day}, {year}"),
            Locale::Es | Locale::Fr | Locale::PtBr => format!("{day} {month_short} {year}"),
            Locale::De => format!("{day}. {month_full} {year}"),
            Locale::Ru => format!("{day} {month_full} {year}"),
            _ => format!("{year}/{month_num}/{day}"),
        },
        (_, DateStyle::Long) => match locale {
            Locale::En => format!("{month_full} {day}, {year}"),
            Locale::Es | Locale::Fr | Locale::PtBr => format!("{day} {month_full} {year}"),
            Locale::De => format!("{day}. {month_full} {year}"),
            Locale::Ru => format!("{day} {month_full} {year}"),
            Locale::ZhHans => format!("{year}年{month_num}月{day}日"),
            Locale::ZhHant => format!("{year}年{month_num}月{day}日"),
            Locale::Ja => format!("{year}年{month_num}月{day}日"),
            Locale::Ko => format!("{year}년 {month_num}월 {day}일"),
        },
        (_, DateStyle::Full) => {
            let weekday = weekday_names(locale)[weekday_index(at.weekday())];
            match locale {
                Locale::En => format!("{weekday}, {month_full} {day}, {year}"),
                Locale::Es | Locale::Fr | Locale::PtBr => {
                    format!("{weekday}, {day} {month_full} {year}")
                }
                Locale::De => format!("{weekday}, {day}. {month_full} {year}"),
                Locale::Ru => format!("{weekday}, {day} {month_full} {year}"),
                Locale::ZhHans => format!("{year}年{month_num}月{day}日 {weekday}"),
                Locale::ZhHant => format!("{year}年{month_num}月{day}日 {weekday}"),
                Locale::Ja => format!("{year}年{month_num}月{day}日 {weekday}"),
                Locale::Ko => format!("{year}년 {month_num}월 {day}일 {weekday}"),
            }
        }
    }
}

/// Format a timestamp as a local time in `locale`.
pub fn format_time(locale: Locale, at: &DateTime<Utc>, style: TimeStyle) -> String {
    let hour = format_integer_plain(locale, i64::from(at.hour()));
    let minute = format_integer_padded(locale, i64::from(at.minute()), 2);
    match style {
        TimeStyle::Short => format!("{hour}:{minute}"),
        TimeStyle::Medium | TimeStyle::Full => {
            let second = format_integer_padded(locale, i64::from(at.second()), 2);
            format!("{hour}:{minute}:{second}")
        }
    }
}

/// Format a timestamp as a localized date and time.
pub fn format_datetime(
    locale: Locale,
    at: &DateTime<Utc>,
    date_style: DateStyle,
    time_style: TimeStyle,
) -> String {
    let date = format_date(locale, at, date_style);
    let time = format_time(locale, at, time_style);
    match locale {
        // Scripts with no infix separator concatenate directly.
        Locale::ZhHans | Locale::ZhHant | Locale::Ja => format!("{date} {time}"),
        _ => format!("{date} {time}"),
    }
}

/// Relative-time unit keys, for callers that build `"{n} minutes ago"` style
/// messages from the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeUnit {
    /// Effectively "just now" — under ten seconds.
    Now,
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days (and, past thirty days, months rendered as `day / 30`).
    Day,
}

/// Split an elapsed/remaining span into a `(value, unit)` pair for
/// relative-time catalog messages. `past` distinguishes a past span from a
/// future one at the call site, which is what selects between the
/// `time.past_*` and `time.future_*` catalog keys.
pub fn relative_parts(elapsed_secs: u64, past: bool) -> (u64, RelativeUnit, bool) {
    if elapsed_secs < 10 {
        return (elapsed_secs, RelativeUnit::Now, past);
    }
    if elapsed_secs < 60 {
        return (elapsed_secs, RelativeUnit::Second, past);
    }
    let minutes = elapsed_secs / 60;
    if minutes < 60 {
        return (minutes, RelativeUnit::Minute, past);
    }
    let hours = elapsed_secs / 3_600;
    if hours < 24 {
        return (hours, RelativeUnit::Hour, past);
    }
    let days = elapsed_secs / 86_400;
    if days < 30 {
        return (days, RelativeUnit::Day, past);
    }
    (days / 30, RelativeUnit::Day, past)
}

/// Is `past` meaningful for this unit? `Now` is symmetric.
pub const fn unit_is_past_only(unit: RelativeUnit) -> bool {
    matches!(unit, RelativeUnit::Second | RelativeUnit::Minute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, hh, mm, ss).unwrap()
    }

    #[test]
    fn integers_are_grouped_per_locale() {
        assert_eq!(format_integer(Locale::En, 1_234_567), "1,234,567");
        assert_eq!(format_integer(Locale::De, 1_234_567), "1.234.567");
        assert_eq!(format_integer(Locale::Es, 1_234_567), "1.234.567");
        assert_eq!(format_integer(Locale::Fr, 1_234_567), "1\u{a0}234\u{a0}567");
        assert_eq!(format_integer(Locale::Ru, 1_234_567), "1\u{a0}234\u{a0}567");
        assert_eq!(format_integer(Locale::PtBr, 1_234_567), "1.234.567");
        assert_eq!(format_integer(Locale::Ja, 1_234_567), "1,234,567");
        assert_eq!(format_integer(Locale::Ko, 1_234_567), "1,234,567");
        assert_eq!(format_integer(Locale::ZhHans, 1_234_567), "1,234,567");
        assert_eq!(format_integer(Locale::ZhHant, 1_234_567), "1,234,567");
    }

    #[test]
    fn group_edge_widths_are_correct() {
        assert_eq!(format_integer(Locale::En, 0), "0");
        assert_eq!(format_integer(Locale::En, 7), "7");
        assert_eq!(format_integer(Locale::En, 70), "70");
        assert_eq!(format_integer(Locale::En, 700), "700");
        assert_eq!(format_integer(Locale::En, 7_000), "7,000");
        assert_eq!(format_integer(Locale::En, 70_000), "70,000");
        assert_eq!(format_integer(Locale::En, 700_000), "700,000");
        assert_eq!(format_integer(Locale::En, 7_000_000), "7,000,000");
        assert_eq!(format_integer(Locale::En, -1_234_567), "-1,234,567");
    }

    #[test]
    fn decimals_use_the_locale_mark() {
        assert_eq!(
            format_number(Locale::En, 1234.5, NumberStyle::OneDecimal),
            "1,234.5"
        );
        assert_eq!(
            format_number(Locale::De, 1234.5, NumberStyle::OneDecimal),
            "1.234,5"
        );
        assert_eq!(
            format_number(Locale::Fr, 1234.5, NumberStyle::OneDecimal),
            "1\u{a0}234,5"
        );
        assert_eq!(
            format_number(Locale::En, 0.125, NumberStyle::TwoDecimals),
            "0.13"
        );
        assert_eq!(format_number(Locale::En, 2.0, NumberStyle::Default), "2");
    }

    #[test]
    fn percentages_render_with_a_decimal_mark() {
        assert_eq!(format_percent(Locale::En, 0.912), "91.2%");
        assert_eq!(format_percent(Locale::De, 0.912), "91,2%");
        assert_eq!(format_percent(Locale::En, 1.0), "100.0%");
        assert_eq!(format_percent(Locale::En, 0.0), "0.0%");
    }

    #[test]
    fn byte_units_are_binary_with_correct_thresholds() {
        assert_eq!(format_bytes(Locale::En, 0), "0 B");
        assert_eq!(format_bytes(Locale::En, 512), "512 B");
        assert_eq!(format_bytes(Locale::En, 1023), "1,023 B");
        // The pre-i18n helper divided by 1024 twice and still printed "KB";
        // the thresholds must now actually match the label.
        assert_eq!(format_bytes(Locale::En, 1024), "1 KB");
        assert_eq!(format_bytes(Locale::En, 1536), "1.5 KB");
        assert_eq!(format_bytes(Locale::En, 1024 * 1024), "1 MB");
        assert_eq!(format_bytes(Locale::En, 3 * 1024 * 1024 * 1024), "3 GB");
        assert_eq!(format_bytes(Locale::Ru, 1024 * 1024), "1 МБ");
        assert_eq!(format_bytes(Locale::Fr, 1024 * 1024), "1 MB");
    }

    #[test]
    fn durations_use_locale_units_and_order() {
        assert_eq!(format_duration(Locale::En, 0), "<1s");
        assert_eq!(format_duration(Locale::En, 45), "45s");
        assert_eq!(format_duration(Locale::En, 60), "1min");
        assert_eq!(format_duration(Locale::En, 90), "1min 30s");
        assert_eq!(format_duration(Locale::En, 3_600), "1h");
        assert_eq!(format_duration(Locale::En, 3_302), "55min 2s");
        assert_eq!(format_duration(Locale::En, 7_500), "2h 5min");
        assert_eq!(format_duration(Locale::En, 7_502), "2h 5min 2s");
        assert_eq!(format_duration(Locale::En, 90_061), "1d 1h 1min 1s");
        // CJK attaches the unit and orders hours before minutes.
        assert_eq!(format_duration(Locale::ZhHans, 7_500), "2小时5分钟");
        assert_eq!(format_duration(Locale::ZhHant, 7_500), "2小時5分鐘");
        assert_eq!(format_duration(Locale::Ja, 7_500), "2時間5分");
        assert_eq!(format_duration(Locale::Ko, 7_500), "2시간5분");
        // Russian, German and French keep a space before the unit.
        assert_eq!(format_duration(Locale::Ru, 7_500), "2 ч 5 мин");
        assert_eq!(format_duration(Locale::De, 90), "1 Min. 30 s");
        assert_eq!(format_duration(Locale::Fr, 7_500), "2 h 5 min");
        assert_eq!(format_duration(Locale::Es, 7_500), "2h 5min");
        assert_eq!(format_duration(Locale::PtBr, 7_500), "2h 5min");
    }

    #[test]
    fn date_fields_are_never_grouped() {
        let ts = at(2026, 3, 9, 14, 5, 7);
        for locale in Locale::ALL {
            for style in [
                DateStyle::Short,
                DateStyle::Medium,
                DateStyle::Long,
                DateStyle::Full,
            ] {
                let rendered = format_date(locale, &ts, style);
                assert!(
                    rendered.contains("2026"),
                    "{locale:?} lost or regrouped the year: {rendered}"
                );
            }
        }
        assert_eq!(format_integer_plain(Locale::En, 2_026), "2026");
        assert_eq!(format_integer_padded(Locale::En, 5, 2), "05");
        assert_eq!(format_integer_padded(Locale::En, -5, 2), "-05");
        assert_eq!(format_integer_padded(Locale::En, 123, 2), "123");
    }

    #[test]
    fn duration_parts_round_trip() {
        let parts = DurationParts::from_secs(7_502);
        assert_eq!(parts.days, 0);
        assert_eq!(parts.hours, 2);
        assert_eq!(parts.minutes, 5);
        assert_eq!(parts.seconds, 2);
        assert_eq!(parts.total_secs(), 7_502);
        assert!(DurationParts::from_secs(0).is_zero());
    }

    #[test]
    fn dates_follow_locale_field_order() {
        let ts = at(2026, 3, 9, 14, 5, 7);
        assert_eq!(format_date(Locale::En, &ts, DateStyle::Short), "3/9/2026");
        assert_eq!(format_date(Locale::De, &ts, DateStyle::Short), "9.3.2026");
        assert_eq!(
            format_date(Locale::ZhHans, &ts, DateStyle::Short),
            "2026/3/9"
        );
        assert_eq!(
            format_date(Locale::En, &ts, DateStyle::Medium),
            "Mar 9, 2026"
        );
        assert_eq!(
            format_date(Locale::En, &ts, DateStyle::Long),
            "March 9, 2026"
        );
        assert_eq!(
            format_date(Locale::ZhHans, &ts, DateStyle::Long),
            "2026年3月9日"
        );
        assert_eq!(
            format_date(Locale::Ja, &ts, DateStyle::Long),
            "2026年3月9日"
        );
        assert_eq!(
            format_date(Locale::Ko, &ts, DateStyle::Long),
            "2026년 3월 9일"
        );
        assert_eq!(
            format_date(Locale::Ru, &ts, DateStyle::Long),
            "9 марта 2026"
        );
        // 2026-03-09 is a Monday.
        assert_eq!(
            format_date(Locale::En, &ts, DateStyle::Full),
            "Monday, March 9, 2026"
        );
    }

    #[test]
    fn times_are_zero_padded() {
        let ts = at(2026, 3, 9, 4, 5, 7);
        assert_eq!(format_time(Locale::En, &ts, TimeStyle::Short), "4:05");
        assert_eq!(format_time(Locale::En, &ts, TimeStyle::Medium), "4:05:07");
        assert_eq!(format_time(Locale::Ko, &ts, TimeStyle::Short), "4:05");
    }

    #[test]
    fn non_finite_numbers_never_print_nan_or_inf() {
        assert_eq!(
            format_number(Locale::En, f64::NAN, NumberStyle::Default),
            "\u{2014}"
        );
        assert_eq!(
            format_number(Locale::En, f64::INFINITY, NumberStyle::Default),
            "\u{2014}"
        );
    }

    #[test]
    fn relative_parts_pick_the_right_unit() {
        assert_eq!(relative_parts(3, true), (3, RelativeUnit::Now, true));
        assert_eq!(relative_parts(45, true), (45, RelativeUnit::Second, true));
        assert_eq!(relative_parts(600, true), (10, RelativeUnit::Minute, true));
        assert_eq!(relative_parts(7_200, true), (2, RelativeUnit::Hour, true));
        assert_eq!(
            relative_parts(86_400 * 3, true),
            (3, RelativeUnit::Day, true)
        );
        // Future spans keep the flag so the caller picks the right key.
        assert_eq!(
            relative_parts(600, false),
            (10, RelativeUnit::Minute, false)
        );
        assert!(unit_is_past_only(RelativeUnit::Second));
        assert!(!unit_is_past_only(RelativeUnit::Hour));
    }

    #[test]
    fn name_tables_are_complete_and_non_blank() {
        for locale in Locale::ALL {
            for table in [
                month_names(locale).as_slice(),
                month_names_short(locale).as_slice(),
                weekday_names(locale).as_slice(),
                weekday_names_short(locale).as_slice(),
            ] {
                for entry in table {
                    assert!(!entry.trim().is_empty(), "{locale:?} has a blank name");
                }
            }
            assert_eq!(month_names(locale).len(), 12);
            assert_eq!(month_names_short(locale).len(), 12);
            assert_eq!(weekday_names(locale).len(), 7);
            assert_eq!(weekday_names_short(locale).len(), 7);
        }
    }
}
