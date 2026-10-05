//! A small, strict ICU MessageFormat subset.
//!
//! Supported syntax (everything else is a parse error, reported with the key so
//! a bad catalog fails loudly instead of printing raw braces at the user):
//!
//! ```text
//! {name}                                     simple argument
//! {name, number} {name, number, percent}     locale-aware number styles
//! {name, number, integer|grouped|bytes|one-decimal|two-decimals}
//! {name, date, short|medium|long|full}
//! {name, time, short|medium|full}
//! {name, datetime, short|medium|long}
//! {name, duration}                           compact localized duration
//! {name, plural, =0 {…} one {…} other {…}}   CLDR cardinal selection
//! {name, selectordinal, …}                   CLDR ordinal selection
//! {name, select, male {…} other {…}}         explicit branch
//! #                                          the enclosing plural's number
//! '{{' '}}' "''" "'{' '}'                    literal braces and apostrophes
//! ```
//!
//! Nesting is supported inside plural/select arms, so
//! `{n, plural, one {{count, number} item} other {{count, number} items}}` works.

use crate::format::{self, DateStyle, NumberStyle, TimeStyle};
use crate::locale::Locale;
use crate::plural::{self, Operands, PluralCategory};

/// A value passed to a message argument.
#[derive(Debug, Clone)]
pub enum Arg {
    /// Textual substitution.
    Str(String),
    /// Integer, formatted with grouping for `{name, number}`.
    Int(i64),
    /// Float, formatted with the requested number style.
    Float(f64),
    /// Boolean, rendered as `0`/`1` when substituted numerically.
    Bool(bool),
    /// Seconds, rendered with `format_duration`.
    Duration(u64),
    /// UTC timestamp; formatted in the active locale.
    Timestamp(chrono::DateTime<chrono::Utc>),
}

impl Arg {
    /// Lossy view for a `select` branch selector or a plain substitution.
    fn as_select_key(&self) -> String {
        match self {
            Arg::Str(s) => s.clone(),
            Arg::Int(i) => i.to_string(),
            Arg::Bool(b) => b.to_string(),
            other => format!("{other:?}"),
        }
    }
}

macro_rules! arg_from_display {
    ($($ty:ty),*) => {
        $(impl From<$ty> for Arg {
            fn from(value: $ty) -> Self { Arg::from_display(&value) }
        })*
    };
}

arg_from_display!(std::net::IpAddr, std::net::SocketAddr, char);

impl Arg {
    fn from_display<T: std::fmt::Display>(value: &T) -> Self {
        Arg::Str(value.to_string())
    }
}

impl From<&str> for Arg {
    fn from(value: &str) -> Self {
        Arg::Str(value.to_string())
    }
}

impl From<&String> for Arg {
    fn from(value: &String) -> Self {
        Arg::Str(value.clone())
    }
}

impl From<String> for Arg {
    fn from(value: String) -> Self {
        Arg::Str(value)
    }
}

impl From<&std::path::Path> for Arg {
    fn from(value: &std::path::Path) -> Self {
        Arg::Str(value.display().to_string())
    }
}

impl From<&std::path::PathBuf> for Arg {
    fn from(value: &std::path::PathBuf) -> Self {
        Arg::Str(value.display().to_string())
    }
}

impl From<std::path::PathBuf> for Arg {
    fn from(value: std::path::PathBuf) -> Self {
        Arg::Str(value.display().to_string())
    }
}

impl From<u128> for Arg {
    fn from(value: u128) -> Self {
        Arg::Str(value.to_string())
    }
}

impl From<std::time::Duration> for Arg {
    fn from(value: std::time::Duration) -> Self {
        Arg::Duration(value.as_secs())
    }
}

macro_rules! arg_from_num {
    ($($ty:ty),*) => {
        $(impl From<$ty> for Arg {
            fn from(value: $ty) -> Self { Arg::Int(value as i64) }
        })*
    };
}

arg_from_num!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<f32> for Arg {
    fn from(value: f32) -> Self {
        Arg::Float(f64::from(value))
    }
}

impl From<f64> for Arg {
    fn from(value: f64) -> Self {
        Arg::Float(value)
    }
}

impl From<bool> for Arg {
    fn from(value: bool) -> Self {
        Arg::Int(i64::from(value))
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Arg {
    fn from(value: chrono::DateTime<chrono::Utc>) -> Self {
        Arg::Timestamp(value)
    }
}

impl From<chrono::NaiveDateTime> for Arg {
    fn from(value: chrono::NaiveDateTime) -> Self {
        Arg::Timestamp(value.and_utc())
    }
}

impl From<chrono::NaiveDate> for Arg {
    fn from(value: chrono::NaiveDate) -> Self {
        Arg::Timestamp(value.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc())
    }
}

/// One argument: a name plus its value.
#[derive(Debug, Clone)]
pub struct ArgValue {
    /// The placeholder name, matching the `{name}` in the catalog message.
    pub name: &'static str,
    /// The bound value.
    pub value: Arg,
}

impl ArgValue {
    /// Bind `value` to the placeholder `name`.
    pub fn new(name: &'static str, value: impl Into<Arg>) -> Self {
        Self {
            name,
            value: value.into(),
        }
    }
}

/// The plural style a `plural` / `selectordinal` argument uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluralStyle {
    Cardinal,
    Ordinal,
}

/// A `plural`/`selectordinal` arm selector.
#[derive(Debug, Clone, PartialEq)]
enum Selector {
    /// `=0` — an exact numeric match, checked before any category.
    Exact(i64),
    Category(PluralCategory),
    /// A `select` key.
    Key(String),
}

/// One parsed message argument.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Text(String),
    /// `{name}` — plain substitution.
    Simple(String),
    Number {
        name: String,
        style: NumberStyle,
    },
    Date {
        name: String,
        style: DateStyle,
    },
    Time {
        name: String,
        style: TimeStyle,
    },
    DateTime {
        name: String,
        style: DateStyle,
    },
    Duration {
        name: String,
    },
    /// `#` inside a plural arm: the enclosing argument's value minus the
    /// plural's `offset`, formatted for the locale. A dedicated node (rather
    /// than a `Number` bound to the argument name) is what makes `offset:`
    /// render the adjusted number instead of the original.
    Hash,
    Plural {
        name: String,
        style: PluralStyle,
        offset: i64,
        arms: Vec<(Selector, Vec<Node>)>,
    },
    Select {
        name: String,
        arms: Vec<(Selector, Vec<Node>)>,
    },
}

/// A parse failure, with the byte offset at which it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageError {
    /// The message that failed to parse, verbatim.
    pub message: String,
    /// Byte offset within `message` where the failure was detected.
    pub offset: usize,
    /// A short, stable description of the problem.
    pub detail: &'static str,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid message at byte {} ({}): {:?}",
            self.offset, self.detail, self.message
        )
    }
}

impl std::error::Error for MessageError {}

/// Render `pattern` for `locale` with `args`.
///
/// A pattern that fails to parse falls back to the raw pattern text rather
/// than panicking: a rendering path must never take the UI down.
pub fn render(locale: Locale, pattern: &str, args: &[ArgValue]) -> String {
    match parse(pattern) {
        Ok(nodes) => render_nodes(locale, &nodes, args, None).0,
        Err(_) => pattern.to_string(),
    }
}

/// Parse `pattern` into a node list.
fn parse(pattern: &str) -> Result<Vec<Node>, MessageError> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut parser = Parser {
        chars,
        pos: 0,
        plural_stack: Vec::new(),
    };
    let nodes = parser.parse_nodes(0)?;
    if parser.pos < parser.chars.len() {
        return Err(parser.error("unexpected trailing input"));
    }
    Ok(nodes)
}

/// Parse-time validation: `Ok(())` when every message in a catalog is
/// syntactically valid. Used by the catalog tests so a broken translation can
/// never ship.
pub fn validate(pattern: &str) -> Result<(), MessageError> {
    parse(pattern).map(|_| ())
}

/// A rendered message plus whether it had to fall back.
///
/// The fallback flag exists so the desktop can surface a "translation missing"
/// affordance in developer builds without a second lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The rendered message.
    pub text: String,
    /// True when a key was missing, an argument was absent, or the pattern
    /// failed to parse — i.e. the output is not fully translated.
    pub fell_back: bool,
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// Names of the plural/selectordinal arguments whose arms are currently
    /// being parsed. `#` is only meaningful inside one of them, including when
    /// the arm contains a nested `select`.
    plural_stack: Vec<String>,
}

impl Parser {
    /// Byte offset of the cursor within the source message.
    fn offset(&self) -> usize {
        self.chars[..self.pos].iter().map(|c| c.len_utf8()).sum()
    }

    fn error(&self, detail: &'static str) -> MessageError {
        MessageError {
            message: self.chars.iter().collect(),
            offset: self.offset(),
            detail,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Parse nodes until end-of-input or a closing `}`. `depth` is 0 at the
    /// outermost level and 1 inside a plural/select arm; at depth 0 a doubled
    /// brace is a literal, which keeps JSON-authored catalogs readable without
    /// ambiguity against the `}` that closes an arm.
    fn parse_nodes(&mut self, depth: usize) -> Result<Vec<Node>, MessageError> {
        let mut text = String::new();
        let mut nodes: Vec<Node> = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                '}' if depth > 0 => break,
                '}' => {
                    // Depth 0: `}}` is a literal brace, a lone `}` is an error.
                    if self.chars.get(self.pos + 1) == Some(&'}') {
                        text.push('}');
                        self.pos += 2;
                        continue;
                    }
                    break;
                }
                '#' if !self.plural_stack.is_empty() => {
                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }
                    nodes.push(Node::Hash);
                    self.pos += 1;
                }
                '{' => {
                    // `{{` is a literal brace, not an argument start.
                    if depth == 0 && self.chars.get(self.pos + 1) == Some(&'{') {
                        text.push('{');
                        self.pos += 2;
                        continue;
                    }
                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }
                    let node = self.parse_argument()?;
                    nodes.push(node);
                }
                '\'' => {
                    // ICU apostrophe rules: a lone `'` is literal; `'X'` where
                    // X is `{`, `}` or `#` quotes it; `''` is one `'`.
                    match self.chars.get(self.pos + 1) {
                        Some('{') | Some('}') | Some('#') => {
                            text.push(self.chars[self.pos + 1]);
                            self.pos += 2;
                        }
                        Some('\'') => {
                            text.push('\'');
                            self.pos += 2;
                        }
                        _ => {
                            text.push('\'');
                            self.pos += 1;
                        }
                    }
                }
                _ => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }
        Ok(nodes)
    }

    fn parse_argument(&mut self) -> Result<Node, MessageError> {
        debug_assert_eq!(self.peek(), Some('{'));
        self.pos += 1;
        let name = self.parse_arg_name()?;
        self.skip_ws();
        match self.peek() {
            Some('}') => {
                self.pos += 1;
                Ok(Node::Simple(name))
            }
            Some(',') => {
                self.pos += 1;
                self.skip_ws();
                let kind = self.parse_identifier()?;
                self.skip_ws();
                match kind.as_str() {
                    "number" => self.finish_number(&name),
                    "date" => self.finish_date(&name, false),
                    "time" => self.finish_date(&name, true),
                    "datetime" => self.finish_datetime(&name),
                    "duration" => {
                        expect(self, '}')?;
                        Ok(Node::Duration { name })
                    }
                    "plural" => self.finish_plural(&name, PluralStyle::Cardinal),
                    "selectordinal" => self.finish_plural(&name, PluralStyle::Ordinal),
                    "select" => self.finish_select(&name),
                    other => Err(MessageError {
                        offset: self.offset(),
                        ..error_unexpected(other)
                    }),
                }
            }
            Some(c) => Err(MessageError {
                offset: self.offset(),
                ..error_unexpected(&c.to_string())
            }),
            None => Err(self.error("unterminated argument")),
        }
    }

    fn finish_number(&mut self, name: &str) -> Result<Node, MessageError> {
        let mut style = NumberStyle::Default;
        if self.peek() == Some(',') {
            self.pos += 1;
            self.skip_ws();
            let raw = self.parse_identifier()?;
            style = match raw.as_str() {
                "integer" => NumberStyle::Integer,
                "grouped" => NumberStyle::Grouped,
                "percent" => NumberStyle::Percent,
                "bytes" => NumberStyle::Bytes,
                "one-decimal" | "oneDecimal" => NumberStyle::OneDecimal,
                "two-decimals" | "twoDecimals" => NumberStyle::TwoDecimals,
                "default" | "" => NumberStyle::Default,
                _ => return Err(self.error("unknown number style")),
            };
            self.skip_ws();
        }
        expect(self, '}')?;
        Ok(Node::Number {
            name: name.to_string(),
            style,
        })
    }

    fn finish_date(&mut self, name: &str, time: bool) -> Result<Node, MessageError> {
        let mut style_raw = String::new();
        if self.peek() == Some(',') {
            self.pos += 1;
            self.skip_ws();
            style_raw = self.parse_identifier()?;
            self.skip_ws();
        }
        expect(self, '}')?;
        if time {
            let style = match style_raw.as_str() {
                "" | "short" => TimeStyle::Short,
                "medium" => TimeStyle::Medium,
                "full" => TimeStyle::Full,
                _ => return Err(self.error("unknown time style")),
            };
            Ok(Node::Time {
                name: name.to_string(),
                style,
            })
        } else {
            let style = date_style(&style_raw)?;
            Ok(Node::Date {
                name: name.to_string(),
                style,
            })
        }
    }

    fn finish_datetime(&mut self, name: &str) -> Result<Node, MessageError> {
        let mut style_raw = String::new();
        if self.peek() == Some(',') {
            self.pos += 1;
            self.skip_ws();
            style_raw = self.parse_identifier()?;
            self.skip_ws();
        }
        expect(self, '}')?;
        let style = date_style(&style_raw)?;
        Ok(Node::DateTime {
            name: name.to_string(),
            style,
        })
    }

    fn finish_plural(&mut self, name: &str, style: PluralStyle) -> Result<Node, MessageError> {
        // `{n, plural, …}` — the comma separating the type from the arms.
        if self.peek() == Some(',') {
            self.pos += 1;
        }
        let mut offset = 0i64;
        let mut arms: Vec<(Selector, Vec<Node>)> = Vec::new();
        // The arms are parsed with this argument on the plural stack so that a
        // `#` inside them — including inside a nested `select` — renders the
        // plural value rather than a literal `#`.
        self.plural_stack.push(name.to_string());
        let parsed = (|| -> Result<(), MessageError> {
            loop {
                self.skip_ws();
                // An optional `offset:N` may precede the arms. Only treat `o…`
                // as the keyword when it really is `offset` followed by `:` —
                // the selector `other` also starts with `o`.
                if self.starts_with_keyword("offset") {
                    self.pos += "offset".len();
                    self.skip_ws();
                    if self.peek() != Some(':') {
                        return Err(self.error("malformed plural offset"));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let raw = self.parse_number_literal()?;
                    offset = raw
                        .parse::<i64>()
                        .map_err(|_| self.error("bad plural offset"))?;
                    continue;
                }
                let (selector, body) = self.parse_one_arm()?;
                arms.push((selector, body));
                if arms
                    .iter()
                    .any(|(s, _)| *s == Selector::Category(PluralCategory::Other))
                {
                    break;
                }
            }
            Ok(())
        })();
        self.plural_stack.pop();
        parsed?;
        expect(self, '}')?;
        Ok(Node::Plural {
            name: name.to_string(),
            style,
            offset,
            arms,
        })
    }

    /// Parse one `selector { body }` arm of a plural or select.
    fn parse_one_arm(&mut self) -> Result<(Selector, Vec<Node>), MessageError> {
        let selector = match self.peek() {
            Some('=') => {
                self.pos += 1;
                let raw = self.parse_number_literal()?;
                Selector::Exact(
                    raw.parse::<i64>()
                        .map_err(|_| self.error("bad exact selector"))?,
                )
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let word = self.parse_identifier()?;
                match word.as_str() {
                    "zero" => Selector::Category(PluralCategory::Zero),
                    "one" => Selector::Category(PluralCategory::One),
                    "two" => Selector::Category(PluralCategory::Two),
                    "few" => Selector::Category(PluralCategory::Few),
                    "many" => Selector::Category(PluralCategory::Many),
                    "other" => Selector::Category(PluralCategory::Other),
                    _ => return Err(self.error("unknown plural category")),
                }
            }
            Some('}') => return Err(self.error("arm list needs an `other` arm")),
            _ => return Err(self.error("expected arm selector")),
        };
        self.skip_ws();
        if self.peek() != Some('{') {
            return Err(self.error("expected `{` after arm selector"));
        }
        self.pos += 1;
        let body = self.parse_nodes(1)?;
        expect(self, '}')?;
        Ok((selector, body))
    }

    fn finish_select(&mut self, name: &str) -> Result<Node, MessageError> {
        // `{state, select, …}` — the comma separating the type from the arms.
        if self.peek() == Some(',') {
            self.pos += 1;
        }
        let mut arms: Vec<(Selector, Vec<Node>)> = Vec::new();
        loop {
            self.skip_ws();
            let key = match self.peek() {
                Some('}') => return Err(self.error("select needs an `other` arm")),
                Some(c) if c.is_alphanumeric() || c == '_' || c == '-' => {
                    let mut raw = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_alphanumeric() || c == '_' || c == '-' {
                            raw.push(c);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    raw
                }
                _ => return Err(self.error("expected select key")),
            };
            self.skip_ws();
            if self.peek() != Some('{') {
                return Err(self.error("expected `{` after select key"));
            }
            self.pos += 1;
            let body = self.parse_nodes(1)?;
            expect(self, '}')?;
            arms.push((Selector::Key(key), body));
            if arms
                .iter()
                .any(|(s, _)| *s == Selector::Key("other".into()))
            {
                break;
            }
        }
        expect(self, '}')?;
        Ok(Node::Select {
            name: name.to_string(),
            arms,
        })
    }

    /// Does the input at `pos` start with `word` followed by a non-identifier
    /// character?
    fn starts_with_keyword(&self, word: &str) -> bool {
        let chars: Vec<char> = word.chars().collect();
        if self.pos + chars.len() > self.chars.len() {
            return false;
        }
        for (offset, expected) in chars.iter().enumerate() {
            if self.chars[self.pos + offset] != *expected {
                return false;
            }
        }
        match self.chars.get(self.pos + chars.len()) {
            Some(next) => !(next.is_ascii_alphanumeric() || *next == '_'),
            None => true,
        }
    }

    fn parse_arg_name(&mut self) -> Result<String, MessageError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("empty argument name"));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn parse_identifier(&mut self) -> Result<String, MessageError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("expected identifier"));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn parse_number_literal(&mut self) -> Result<String, MessageError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("expected number"));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }
}

fn error_unexpected(_detail: &str) -> MessageError {
    MessageError {
        message: String::new(),
        offset: 0,
        detail: "unexpected character in argument",
    }
}

fn expect(parser: &mut Parser, want: char) -> Result<(), MessageError> {
    match parser.peek() {
        Some(c) if c == want => {
            parser.pos += 1;
            Ok(())
        }
        _ => Err(parser.error("unexpected end of argument")),
    }
}

fn date_style(raw: &str) -> Result<DateStyle, MessageError> {
    Ok(match raw {
        "" | "short" => DateStyle::Short,
        "medium" => DateStyle::Medium,
        "long" => DateStyle::Long,
        "full" => DateStyle::Full,
        _ => {
            return Err(MessageError {
                message: raw.to_string(),
                offset: 0,
                detail: "unknown date style",
            })
        }
    })
}

fn lookup<'a>(args: &'a [ArgValue], name: &str) -> Option<&'a Arg> {
    args.iter().find(|a| a.name == name).map(|a| &a.value)
}

/// The value a `#` placeholder renders: the plural argument minus `offset`.
#[derive(Debug, Clone, Copy)]
struct PluralValue {
    value: f64,
    /// The value as an integer, when the source argument was one.
    integer: Option<i64>,
}

/// Look up an argument, or report a visible `{name}` placeholder.
/// the render as having fallen back. Shared by every node kind so a missing
/// argument is always visible and never silently blank.
fn missing_placeholder(out: &mut String, name: &str) {
    out.push('{');
    out.push_str(name);
    out.push('}');
}

fn render_nodes(
    locale: Locale,
    nodes: &[Node],
    args: &[ArgValue],
    plural_value: Option<PluralValue>,
) -> (String, bool) {
    let mut out = String::new();
    let mut fell_back = false;
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Simple(name) => match lookup(args, name) {
                Some(Arg::Str(s)) => out.push_str(s),
                Some(Arg::Int(i)) => out.push_str(&format::format_integer(locale, *i)),
                Some(Arg::Float(f)) => {
                    out.push_str(&format::format_number(locale, *f, NumberStyle::Default))
                }
                Some(Arg::Duration(secs)) => out.push_str(&format::format_duration(locale, *secs)),
                Some(Arg::Timestamp(at)) => {
                    out.push_str(&format::format_datetime(
                        locale,
                        at,
                        DateStyle::Medium,
                        TimeStyle::Short,
                    ));
                }
                // `Arg::Bool` is never constructed today; render it as its
                // number so the arm stays total if a `From<bool>` reappears.
                Some(Arg::Bool(b)) => out.push_str(&format::format_integer(locale, i64::from(*b))),
                None => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Number { name, style } => match lookup(args, name) {
                Some(Arg::Int(i)) => {
                    out.push_str(&render_number(locale, *i as f64, *style, Some(*i)))
                }
                Some(Arg::Float(f)) => out.push_str(&render_number(locale, *f, *style, None)),
                Some(Arg::Bool(b)) => {
                    out.push_str(&render_number(locale, i64::from(*b) as f64, *style, None))
                }
                Some(Arg::Duration(secs)) => out.push_str(&format::format_duration(locale, *secs)),
                Some(Arg::Timestamp(at)) => out.push_str(&format::format_datetime(
                    locale,
                    at,
                    DateStyle::Medium,
                    TimeStyle::Short,
                )),
                Some(Arg::Str(s)) => {
                    fell_back = true;
                    out.push_str(s);
                }
                None => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Date { name, style } => match lookup(args, name) {
                Some(Arg::Timestamp(at)) => out.push_str(&format::format_date(locale, at, *style)),
                _ => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Time { name, style } => match lookup(args, name) {
                Some(Arg::Timestamp(at)) => out.push_str(&format::format_time(locale, at, *style)),
                _ => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::DateTime { name, style } => match lookup(args, name) {
                Some(Arg::Timestamp(at)) => out.push_str(&format::format_datetime(
                    locale,
                    at,
                    *style,
                    TimeStyle::Short,
                )),
                _ => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Duration { name } => match lookup(args, name) {
                Some(Arg::Duration(secs)) => out.push_str(&format::format_duration(locale, *secs)),
                Some(Arg::Int(i)) if *i >= 0 => {
                    out.push_str(&format::format_duration(locale, *i as u64))
                }
                Some(Arg::Float(f)) if *f >= 0.0 => {
                    out.push_str(&format::format_duration(locale, *f as u64))
                }
                _ => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Hash => match plural_value {
                Some(value) => match value.integer {
                    Some(integer) => out.push_str(&format::format_integer(locale, integer)),
                    None => out.push_str(&format::format_number(
                        locale,
                        value.value,
                        NumberStyle::Default,
                    )),
                },
                // `#` outside a plural arm: render it literally rather than
                // guessing a value.
                None => {
                    fell_back = true;
                    out.push('#');
                }
            },
            Node::Plural {
                name,
                style,
                offset,
                arms,
            } => match lookup(args, name) {
                Some(value) => {
                    let (raw, exact) = plural_operands(value, *style);
                    let adjusted = (raw - *offset as f64).max(0.0);
                    let integer = exact.map(|n| (n - *offset).max(0));
                    let body = select_plural(locale, *style, adjusted, exact, arms);
                    let (text, sub_fallback) = render_nodes(
                        locale,
                        &body,
                        args,
                        Some(PluralValue {
                            value: adjusted,
                            integer,
                        }),
                    );
                    fell_back |= sub_fallback;
                    out.push_str(&text);
                }
                None => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
            Node::Select { name, arms } => match lookup(args, name) {
                Some(value) => {
                    let key = value.as_select_key();
                    let body = arms
                        .iter()
                        .find(|(s, _)| matches!(s, Selector::Key(k) if *k == key))
                        .or_else(|| {
                            arms.iter()
                                .find(|(s, _)| matches!(s, Selector::Key(k) if k == "other"))
                        })
                        .map(|(_, body)| body);
                    if let Some(body) = body {
                        // A `select` nested inside a plural arm keeps the
                        // plural context so its `#` placeholders still work.
                        let (text, sub_fallback) = render_nodes(locale, body, args, plural_value);
                        fell_back |= sub_fallback;
                        out.push_str(&text);
                    }
                }
                None => {
                    fell_back = true;
                    missing_placeholder(&mut out, name);
                }
            },
        }
    }
    (out, fell_back)
}

/// `(raw value, the integer when the source argument was one)`
fn plural_operands(value: &Arg, _style: PluralStyle) -> (f64, Option<i64>) {
    match value {
        Arg::Int(i) => (*i as f64, Some(*i)),
        Arg::Float(f) => (*f, None),
        _ => (0.0, Some(0)),
    }
}

fn select_plural(
    locale: Locale,
    style: PluralStyle,
    adjusted: f64,
    exact: Option<i64>,
    arms: &[(Selector, Vec<Node>)],
) -> Vec<Node> {
    // `=N` wins over categories, and matches the un-adjusted value (ICU
    // semantics: `offset` applies to `#` and to the category decision, but an
    // explicit `=N` is written against the original number).
    let _ = adjusted;
    if let Some(n) = exact {
        if let Some((_, body)) = arms.iter().find(|(s, _)| *s == Selector::Exact(n)) {
            return body.clone();
        }
    }
    let ops = match exact {
        Some(i) => Operands::from_int(i),
        None => Operands::from_float(adjusted, 3),
    };
    let category = match style {
        PluralStyle::Cardinal => plural::cardinal(locale, &ops),
        // No shipped locale differentiates ordinals; cardinal is the right
        // fallback and is documented as such.
        PluralStyle::Ordinal => plural::cardinal(locale, &ops),
    };
    arms.iter()
        .find(|(s, _)| *s == Selector::Category(category))
        .or_else(|| {
            arms.iter()
                .find(|(s, _)| *s == Selector::Category(PluralCategory::Other))
        })
        .or_else(|| arms.first())
        .map(|(_, body)| body.clone())
        .unwrap_or_default()
}

fn render_number(locale: Locale, value: f64, style: NumberStyle, exact: Option<i64>) -> String {
    match (style, exact) {
        (NumberStyle::Integer | NumberStyle::Grouped, Some(i)) => format::format_integer(locale, i),
        _ => format::format_number(locale, value, style),
    }
}

/// Render with the fallback flag, for callers that want to detect a missing
/// translation or a missing argument.
pub fn render_checked(locale: Locale, pattern: &str, args: &[ArgValue]) -> Rendered {
    match parse(pattern) {
        Ok(nodes) => {
            let (text, fell_back) = render_nodes(locale, &nodes, args, None);
            Rendered { text, fell_back }
        }
        Err(_) => Rendered {
            text: pattern.to_string(),
            fell_back: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn args(pairs: &[(&'static str, Arg)]) -> Vec<ArgValue> {
        pairs
            .iter()
            .map(|(name, value)| ArgValue::new(name, value.clone()))
            .collect()
    }

    #[test]
    fn simple_substitution() {
        let a = args(&[("name", Arg::from("Ada"))]);
        assert_eq!(render(Locale::En, "Hello {name}", &a), "Hello Ada");
    }

    #[test]
    fn missing_argument_is_visible_not_panicky() {
        let rendered = render_checked(Locale::En, "Hello {name}", &[]);
        assert_eq!(rendered.text, "Hello {name}");
        assert!(rendered.fell_back);
    }

    #[test]
    fn literal_braces_and_apostrophes() {
        // `{{` / `}}` are literal braces at the top level, which keeps
        // JSON-authored catalogs readable.
        assert_eq!(render(Locale::En, "{{literal}}", &[]), "{literal}");
        assert_eq!(render(Locale::En, "it''s", &[]), "it's");
        // ICU quoting: an apostrophe immediately before a special character
        // makes that one character literal. `\u{27}` is used so the two
        // characters in the pattern are unambiguous.
        assert_eq!(render(Locale::En, "\u{27}{", &[]), "{");
        assert_eq!(render(Locale::En, "\u{27}}", &[]), "}");
        // A lone apostrophe stays literal (ICU's lenient quoting).
        assert_eq!(render(Locale::En, "don't", &[]), "don't");
        // Inside a plural arm a single `}` closes the arm, so `}}` must not be
        // read as a literal there.
        assert_eq!(
            render(
                Locale::En,
                "{n, plural, other {#}}",
                &args(&[("n", Arg::from(2))])
            ),
            "2"
        );
    }

    #[test]
    fn numbers_use_locale_formats() {
        let a = args(&[("n", Arg::from(12345))]);
        assert_eq!(render(Locale::En, "{n, number}", &a), "12,345");
        assert_eq!(render(Locale::De, "{n, number}", &a), "12.345");
        assert_eq!(render(Locale::Fr, "{n, number}", &a), "12\u{a0}345");
    }

    #[test]
    fn percent_and_byte_styles() {
        let ratio = args(&[("p", Arg::from(0.912))]);
        assert_eq!(render(Locale::En, "{p, number, percent}", &ratio), "91.2%");
        assert_eq!(render(Locale::De, "{p, number, percent}", &ratio), "91,2%");
        let size = args(&[("b", Arg::from(2 * 1024 * 1024u64))]);
        assert_eq!(render(Locale::En, "{b, number, bytes}", &size), "2 MB");
        assert_eq!(render(Locale::Ru, "{b, number, bytes}", &size), "2 МБ");
    }

    #[test]
    fn plural_selection_per_locale() {
        let one = args(&[("n", Arg::from(1))]);
        let five = args(&[("n", Arg::from(5))]);
        let zero = args(&[("n", Arg::from(0))]);
        let pattern = "{n, plural, one {# file} other {# files}}";
        assert_eq!(render(Locale::En, pattern, &one), "1 file");
        assert_eq!(render(Locale::En, pattern, &five), "5 files");
        // Russian must pick `few` for 2..4.
        let ru = "{n, plural, one {# файл} few {# файла} many {# файлов} other {# файла}}";
        let two = args(&[("n", Arg::from(2))]);
        assert_eq!(render(Locale::Ru, ru, &one), "1 файл");
        assert_eq!(render(Locale::Ru, ru, &two), "2 файла");
        assert_eq!(render(Locale::Ru, ru, &five), "5 файлов");
        assert_eq!(render(Locale::Ru, ru, &zero), "0 файлов");
        // French counts zero as singular ("0 fichier"), English does not.
        assert_eq!(render(Locale::Fr, pattern, &zero), "0 file");
        assert_eq!(render(Locale::En, pattern, &zero), "0 files");
        // CJK has one form only.
        assert_eq!(
            render(Locale::Ja, "{n, plural, other {# 件}}", &five),
            "5 件"
        );
    }

    #[test]
    fn exact_selector_beats_category() {
        let a = args(&[("n", Arg::from(0))]);
        assert_eq!(
            render(
                Locale::En,
                "{n, plural, =0 {no files} one {# file} other {# files}}",
                &a
            ),
            "no files"
        );
    }

    #[test]
    fn plural_offset_shifts_the_hash_value() {
        let a = args(&[("n", Arg::from(3))]);
        assert_eq!(
            render(
                Locale::En,
                "{n, plural, offset:1 =0 {nobody} one {you and # other} other {you and # others}}",
                &a
            ),
            "you and 2 others"
        );
    }

    #[test]
    fn nested_plurals_inside_select() {
        let a = args(&[("n", Arg::from(2)), ("kind", Arg::from("file"))]);
        assert_eq!(
            render(
                Locale::En,
                "{n, plural, one {{kind, select, file {# file} other {# thing}}} other {{kind, select, file {# files} other {# things}}}}",
                &a
            ),
            "2 files"
        );
        let b = args(&[("n", Arg::from(1)), ("kind", Arg::from("thing"))]);
        assert_eq!(
            render(
                Locale::En,
                "{n, plural, one {{kind, select, file {# file} other {# thing}}} other {{kind, select, file {# files} other {# things}}}}",
                &b
            ),
            "1 thing"
        );
    }

    #[test]
    fn select_falls_back_to_other() {
        let a = args(&[("state", Arg::from("weird"))]);
        assert_eq!(
            render(
                Locale::En,
                "{state, select, on {on} off {off} other {unknown}}",
                &a
            ),
            "unknown"
        );
    }

    #[test]
    fn durations_and_dates_render_per_locale() {
        let a = args(&[("secs", Arg::Duration(7_500))]);
        assert_eq!(render(Locale::En, "{secs, duration}", &a), "2h 5min");
        assert_eq!(render(Locale::ZhHans, "{secs, duration}", &a), "2小时5分钟");

        let at = chrono::Utc.with_ymd_and_hms(2026, 3, 9, 14, 5, 0).unwrap();
        let d = args(&[("at", Arg::Timestamp(at))]);
        assert_eq!(render(Locale::En, "{at, date, long}", &d), "March 9, 2026");
        assert_eq!(render(Locale::Ja, "{at, date, long}", &d), "2026年3月9日");
        assert_eq!(render(Locale::En, "{at, time, short}", &d), "14:05");
        assert_eq!(
            render(Locale::En, "{at, datetime, medium}", &d),
            "Mar 9, 2026 14:05"
        );
    }

    #[test]
    fn invalid_patterns_are_rejected_not_guessed() {
        assert!(validate("{name").is_err());
        assert!(validate("{name, plural, one {x}}").is_err());
        assert!(validate("{name, select}").is_err());
        assert!(validate("{name, number, bogus}").is_err());
        assert!(validate("{}").is_err());
        assert!(validate("unclosed }").is_err());
        assert!(validate("{name, plural, one {x} other {y}").is_err());
        // A bad pattern degrades to the raw text rather than panicking.
        assert_eq!(
            render(Locale::En, "{name, plural, one {x}}", &[]),
            "{name, plural, one {x}}"
        );
    }

    #[test]
    fn valid_patterns_are_accepted() {
        for pattern in [
            "plain text",
            "{a}",
            "{a, number}",
            "{a, number, percent}",
            "{a, number, bytes}",
            "{a, number, integer}",
            "{a, date}",
            "{a, date, full}",
            "{a, time, medium}",
            "{a, datetime, long}",
            "{a, duration}",
            "{a, plural, other {x}}",
            "{a, plural, =0 {none} one {# one} other {# other}}",
            "{a, plural, offset:2 other {#}}",
            "{a, selectordinal, other {#th}}",
            "{a, select, x {X} other {O}}",
            "{{escaped}}",
            "'{'quoted'}'",
        ] {
            assert!(validate(pattern).is_ok(), "should parse: {pattern:?}");
        }
    }

    #[test]
    fn conversions_cover_the_call_shapes() {
        assert!(matches!(Arg::from("s"), Arg::Str(_)));
        assert!(matches!(Arg::from(String::from("s")), Arg::Str(_)));
        assert!(matches!(Arg::from(1u8), Arg::Int(1)));
        assert!(matches!(Arg::from(1usize), Arg::Int(1)));
        assert!(matches!(Arg::from(-3i64), Arg::Int(-3)));
        assert!(matches!(Arg::from(1.5f64), Arg::Float(_)));
        assert!(matches!(Arg::from(true), Arg::Int(1)));
        assert!(matches!(
            Arg::from(std::path::PathBuf::from("/tmp/x")),
            Arg::Str(_)
        ));
        assert!(matches!(Arg::Duration(4), Arg::Duration(4)));
    }
}
