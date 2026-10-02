//! Drag-selectable static text for the session timeline.
//!
//! GPUI has no built-in selectable label, so this module owns both sides of
//! the feature:
//! - [`SelectableText`]: a custom element that shapes one source line, wraps
//!   it greedily at the painted width, and paints text + selection highlight.
//!   Rendering and hit-testing share one shaping pass, so the highlight can
//!   never drift from the visible glyphs.
//! - Selection state on [`WorkspaceView`] (`timeline_text_*` fields) plus
//!   container-level mouse handlers (position → block lookup) and the
//!   `Ctrl+C` copy action.
//!
//! Row clicks keep working through a press threshold: clickable rows record
//! the press on mouse-down and act on mouse-up only when the pointer stayed
//! within a few pixels, so text drags never trigger navigation or toggles.

use gpui::*;
use std::ops::Range;
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;

use crate::workspace::WorkspaceView;

actions!(timeline_text, [CopyTimelineSelection]);

/// Key context for selectable-text surfaces. The `Ctrl+C` binding is scoped
/// to it so prompt/file inputs keep their own copy behavior untouched.
pub const TIMELINE_TEXT_CONTEXT: &str = "TimelineText";

/// Press/release distance below which a press counts as a click.
const TAP_DRAG_THRESHOLD_PX: f32 = 6.0;

/// Width guess (px) for the very first height estimate before real
/// measurement. Corrected on the next frame via [`Window::refresh`].
const FIRST_ESTIMATE_WIDTH: f32 = 480.0;

/// Selection highlight, shared with the prompt input convention.
fn selection_color() -> Rgba {
    rgba(0x3a5a8acc)
}

#[derive(Clone, Debug, Default)]
pub struct TimelineTextSelection {
    pub anchor_block: String,
    pub anchor_offset: usize,
    pub focus_block: String,
    pub focus_offset: usize,
}

#[derive(Clone)]
pub struct TextBlockLayout {
    pub origin: Point<Pixels>,
    pub width: Pixels,
    pub line_height: Pixels,
    pub align: TextAlign,
    pub text_hash: u64,
    pub text: String,
    pub fragments: Vec<TextFragment>,
    /// Byte ranges carrying clickable URLs (link/image runs).
    pub links: Vec<(Range<usize>, String)>,
}

/// Horizontal offset of a fragment origin under the block alignment.
fn align_offset(align: TextAlign, block_width: Pixels, frag_width: Pixels) -> Pixels {
    let slack = block_width - frag_width;
    if slack <= px(0.) {
        return px(0.);
    }
    match align {
        TextAlign::Left => px(0.),
        TextAlign::Center => slack * 0.5,
        TextAlign::Right => slack,
    }
}

#[derive(Clone)]
pub struct TextFragment {
    pub start: usize,
    pub end: usize,
    pub line: ShapedLine,
}

#[derive(Clone)]
pub struct TimelineRowPress {
    pub id: String,
    pub pos: Point<Pixels>,
}

#[derive(Clone)]
pub struct TimelineTextPress {
    pub block: String,
    pub anchor: usize,
}

/// Deferred row click: runs on mouse-up only when the press did not drag.
pub type TapCallback = Rc<dyn Fn(&mut WorkspaceView, &mut Window, &mut Context<WorkspaceView>)>;

/// One styled run inside a selectable block (byte length tiles the text).
#[derive(Clone)]
pub struct SelRun {
    pub len: usize,
    pub font: Font,
    pub color: Hsla,
    pub underline: bool,
    pub background: Option<Hsla>,
    pub link: Option<String>,
    pub strike: bool,
}

/// Line-level style shared by all runs of one selectable element.
#[derive(Clone)]
pub struct SelectableStyle {
    pub size: Pixels,
    pub line_height: Pixels,
    pub color: Rgba,
    pub weight: FontWeight,
    pub italic: bool,
    pub family: SharedString,
    pub align: TextAlign,
}

/// Pure conversion: markdown inline runs → plain text + shaped runs.
/// Fully unit-testable (no GPUI context needed).
pub fn build_runs(
    inline: &[crate::markdown::InlineRun],
    theme: &crate::theme::ThemeTokens,
    style: &SelectableStyle,
) -> (String, Vec<SelRun>) {
    use crate::markdown::InlineStyle as MdStyle;
    let mono_family = crate::theme::font_mono(theme);
    let code_bg: Hsla = theme.markdown.code_bg.into();
    let mut text = String::new();
    let mut runs = Vec::with_capacity(inline.len());
    for run in inline {
        let mut family = style.family.clone();
        let mut weight = style.weight;
        let mut italic = style.italic;
        let mut color = style.color;
        let mut underline = false;
        let mut background = None;
        let mut strike = false;
        let mut link = None;
        for s in &run.styles {
            match s {
                MdStyle::Normal => {}
                MdStyle::Strong => {
                    if weight < FontWeight::SEMIBOLD {
                        weight = FontWeight::SEMIBOLD;
                    }
                }
                MdStyle::Emphasis => italic = true,
                MdStyle::Code => {
                    family = mono_family.clone();
                    background = Some(code_bg);
                }
                MdStyle::Link => {
                    color = theme.markdown.link;
                    underline = true;
                }
                MdStyle::Strike => strike = true,
                MdStyle::Image => {
                    color = theme.markdown.link;
                }
            }
        }
        if let Some(url) = run.url.clone() {
            link = Some(url);
        }
        let mut font = font(family);
        font.weight = weight;
        font.style = if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        runs.push(SelRun {
            len: run.text.len(),
            font,
            color: color.into(),
            underline,
            background,
            link,
            strike,
        });
        text.push_str(&run.text);
    }
    (text, runs)
}

/// Build a [`SelectableText`] from markdown inline runs.
pub fn selectable_text(
    view: &Entity<WorkspaceView>,
    block: String,
    inline: &[crate::markdown::InlineRun],
    theme: &crate::theme::ThemeTokens,
    style: &SelectableStyle,
    wrap: bool,
) -> SelectableText {
    let (text, runs) = build_runs(inline, theme, style);
    SelectableText {
        view: view.clone(),
        block: block.into(),
        text: text.into(),
        runs,
        font_size: style.size,
        line_height: style.line_height,
        align: style.align,
        wrap,
    }
}

/// Plain single-style selectable text (tool rows, headers, summaries).
#[allow(clippy::too_many_arguments)]
pub fn selectable_plain(
    view: &Entity<WorkspaceView>,
    block: String,
    text: String,
    font: Font,
    color: Rgba,
    size: Pixels,
    line_height: Pixels,
    wrap: bool,
) -> SelectableText {
    SelectableText {
        view: view.clone(),
        block: block.into(),
        text: text.clone().into(),
        runs: vec![SelRun {
            len: text.len(),
            font,
            color: color.into(),
            underline: false,
            background: None,
            link: None,
            strike: false,
        }],
        font_size: size,
        line_height,
        align: TextAlign::Left,
        wrap,
    }
}

/// FNV-1a hash for text-change detection.
fn text_hash(text: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Fullwidth / wide character: occupies two terminal columns.
///
/// Drives both wrap breaking (`is_cjk`) and column-width measurement in
/// [`crate::markdown`] when re-joining provider hard wrapping.
pub(crate) fn is_wide_char(ch: char) -> bool {
    matches!(ch,
        '\u{1100}'..='\u{115F}' // Hangul Jamo
        | '\u{2000}'..='\u{206F}' // General punctuation: — … “ ” ‘ ’ · ～
        | '\u{2190}'..='\u{21FF}' // Arrows: → ⇒
        | '\u{2600}'..='\u{27BF}' // Symbols/dingbats: ❌ ✅ ● ◆ … (emoji-safe breakable)
        | '\u{2E80}'..='\u{2EFF}' // CJK radicals supplement
        | '\u{2F00}'..='\u{2FDF}' // Kangxi radicals
        | '\u{3000}'..='\u{303F}' // CJK symbols: 、 。 〈 〉 《 》 「 」 『 』 …
        | '\u{3040}'..='\u{30FF}' // Hiragana/Katakana
        | '\u{3100}'..='\u{312F}' // Bopomofo
        | '\u{31A0}'..='\u{31BF}' // Bopomofo extended
        | '\u{3200}'..='\u{32FF}' // Enclosed CJK
        | '\u{3400}'..='\u{4DBF}' // Ext A
        | '\u{4E00}'..='\u{9FFF}' // Unified ideographs
        | '\u{A000}'..='\u{A4CF}' // Yi
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{F900}'..='\u{FAFF}' // Compatibility ideographs
        | '\u{FE10}'..='\u{FE1F}' // Vertical forms
        | '\u{FE30}'..='\u{FE4F}' // Compatibility forms
        | '\u{FF00}'..='\u{FFEF}' // Fullwidth ASCII/punct
        | '\u{1F300}'..='\u{1FAFF}' // Emoji & symbols
        | '\u{20000}'..='\u{2FA1F}') // Ext B–F + compat supplement
}

/// A wide character is also a legal line-break opportunity (CJK typography
/// breaks between ideographs). ASCII breaks come from whitespace/punctuation
/// rules in [`breakable_before`].
fn is_cjk(ch: char) -> bool {
    is_wide_char(ch)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// CJK-aware closing punctuation that must not start a wrapped line.
fn is_closing_punct(ch: char) -> bool {
    matches!(
        ch,
        '。' | '、'
            | '，'
            | '．'
            | '；'
            | '：'
            | '？'
            | '！'
            | '）'
            | '》'
            | '」'
            | '』'
            | '〉'
            | '〕'
            | '】'
            | '’'
            | '”'
            | '…'
            | '—'
            | '～'
            | '→'
            | '·'
            | ','
            | '.'
            | ';'
            | ':'
            | '?'
            | '!'
            | ')'
            | ']'
            | '}'
            | '>'
            | '%'
            | '‰'
            | '°'
    )
}

/// Opening punctuation that must not end a wrapped line.
fn is_opening_punct(ch: char) -> bool {
    matches!(
        ch,
        '（' | '《'
            | '「'
            | '『'
            | '〈'
            | '〔'
            | '【'
            | '('
            | '['
            | '{'
            | '<'
            | '“'
            | '‘'
            | '￥'
            | '$'
            | '—'
    )
}

/// ASCII / path punctuation after which a line break is preferred over a
/// mid-word split: `tool-registry.ts:34` prefers `tool-` + `registry…`,
/// `a/b` prefers `a/` + `b`, `snake_case` prefers `snake_` + `case`.
/// Only consulted after the open/close guards above, so `)` / `.` never
/// start a line when a cleaner break exists; the overlong fallback still
/// force-splits when nothing fits.
fn is_break_after_punct(ch: char) -> bool {
    matches!(
        ch,
        '-' | '/'
            | '_'
            | '.'
            | ':'
            | ';'
            | ','
            | '?'
            | '!'
            | '='
            | '+'
            | '*'
            | '|'
            | '&'
            | '@'
            | '#'
            | '%'
            | '~'
            | '\\'
            | '—'
            | '…'
            | '→'
            | '·'
    )
}

/// Heuristic combining-mark (extender) ranges for grapheme flooring.
fn is_grapheme_extender(ch: char) -> bool {
    matches!(
        ch,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// Floor a byte index to a grapheme-cluster boundary.
///
/// A char boundary between a base char and its combining mark (e.g. offset 4
/// in `"e\u{301}"`) is *inside* the cluster, so flooring must return the
/// cluster start (3), not the boundary itself. Only end-of-text stays put
/// (a cursor may rest after the final grapheme).
pub fn snap_to_grapheme(text: &str, mut ix: usize) -> usize {
    ix = ix.min(text.len());
    if ix == text.len() {
        return ix;
    }
    // Inside a multibyte char: scan back to the previous char boundary.
    while ix > 0 && !text.is_char_boundary(ix) {
        ix -= 1;
    }
    loop {
        // Walk back over trailing extenders (ix sits after them).
        let mut start = ix;
        while start > 0 {
            match text[..start].chars().next_back() {
                Some(ch) if is_grapheme_extender(ch) => start -= ch.len_utf8(),
                _ => break,
            }
        }
        if start != ix {
            ix = start;
            continue;
        }
        // No trailing extenders: if the char AT ix is itself an extender, ix
        // lies strictly inside the previous base char's cluster → snap to it.
        match text[ix..].chars().next() {
            Some(ch) if is_grapheme_extender(ch) => {
                return match text[..ix].chars().next_back() {
                    Some(base) => ix - base.len_utf8(),
                    None => 0,
                };
            }
            _ => return ix,
        }
    }
}

/// Normalize a selection range (ordered, grapheme-snapped, clamped).
pub fn normalize_range(text: &str, a: usize, b: usize) -> Range<usize> {
    let (mut start, mut end) = if a <= b { (a, b) } else { (b, a) };
    start = snap_to_grapheme(text, start).min(text.len());
    end = snap_to_grapheme(text, end).min(text.len());
    start..end
}

/// Double-click word bounds at a byte offset.
pub fn word_bounds_at(text: &str, ix: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    // Clamp BEFORE snapping: `len - 1` may sit inside a multibyte char
    // (e.g. text ending with '）' bytes 54..57, `len - 1 == 56`), so
    // snapping after clamping is required to guarantee a char boundary.
    // The old order (snap then `min(len - 1)`) reintroduced a mid-char
    // index and panicked at `text[ix..]` — fatal here because selection
    // runs inside a non-unwinding GPUI callback (process abort).
    let ix = snap_to_grapheme(text, ix.min(text.len().saturating_sub(1).max(0)));
    if !text.is_char_boundary(ix) {
        return 0..0;
    }
    let ch = text.get(ix..).and_then(|s| s.chars().next());
    match ch {
        Some(ch) if is_word_char(ch) && !is_cjk(ch) => {
            let mut start = ix;
            while start > 0 {
                match text[..start].chars().next_back() {
                    Some(prev) if is_word_char(prev) && !is_cjk(prev) => {
                        start -= prev.len_utf8();
                    }
                    _ => break,
                }
            }
            let mut end = ix;
            while end < text.len() {
                match text[end..].chars().next() {
                    Some(next) if is_word_char(next) && !is_cjk(next) => {
                        end += next.len_utf8();
                    }
                    _ => break,
                }
            }
            start..end
        }
        // CJK or symbol: single grapheme.
        _ => {
            let start = ix;
            let end = text
                .graphemes(true)
                .scan(0usize, |offset, g| {
                    let s = *offset;
                    *offset += g.len();
                    Some((s, *offset))
                })
                .find(|(s, _)| *s >= ix)
                .map(|(_, e)| e)
                .unwrap_or(text.len());
            start..end
        }
    }
}

/// Can a wrapped line break before byte offset `at` (grapheme boundary)?
fn breakable_before(text: &str, at: usize) -> bool {
    if at == 0 || at >= text.len() {
        return false;
    }
    let prev = text[..at].chars().next_back();
    let next = text[at..].chars().next();
    match (prev, next) {
        (Some(p), Some(n)) => {
            // Break after whitespace (space stays at end of previous line).
            if p.is_whitespace() {
                return true;
            }
            // Never start a line with whitespace itself.
            if n.is_whitespace() {
                return false;
            }
            // 避头尾: closing punctuation must not begin a line; opening
            // punctuation must not end one. Breaking here would strand `。`
            // alone on its own line (the reported orphan).
            if is_closing_punct(n) || is_opening_punct(p) {
                return false;
            }
            if is_cjk(p) || is_cjk(n) {
                return true;
            }
            // Prefer breaking after path/code punctuation over mid-word
            // splits: `tool-` + `registry`, `a/` + `b`, `snake_` + `case`.
            if is_break_after_punct(p) {
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Greedy wrap of one shaped source line into byte ranges (grapheme-safe).
///
/// Two invariants beyond plain greedy filling:
/// 1. **No forbidden break** — a break is only taken where
///    [`breakable_before`] allows it. Closing punctuation never starts a
///    line and opening punctuation never ends one.
/// 2. **No single-grapheme stall** — when the fitted range contains no legal
///    break (an unbreakable token wider than the column), the line is split at
///    the largest fitting boundary instead of collapsing to one grapheme per
///    line, which rendered as a vertical single-character column.
fn wrap_ranges(line: &ShapedLine, text: &str, width: Pixels) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return Vec::new();
    }
    if line.width() <= width || width <= px(0.) {
        return vec![(0, text.len())];
    }
    let bounds: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let x = |i: usize| line.x_for_index(i);
    let mut out = Vec::new();
    let mut si = 0usize;
    while bounds[si] < text.len() {
        let start = bounds[si];
        let base = x(start);
        // Binary search: largest boundary fitting the width.
        let mut lo = si + 1;
        let mut hi = bounds.len() - 1;
        let mut best = si;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            if x(bounds[mid]) - base <= width {
                best = mid;
                lo = mid + 1;
            } else if mid == 0 {
                break;
            } else {
                hi = mid - 1;
            }
        }
        // Snap back to the nearest legal break inside the fitted range.
        let fitted = best.max(si + 1);
        let mut bi = fitted;
        while bi > si + 1 && !breakable_before(text, bounds[bi]) {
            bi -= 1;
        }
        // No legal break inside the fitted range (overlong unbreakable
        // token): split at the full fitted width rather than stalling to a
        // single grapheme per line.
        if bi == si + 1 && !breakable_before(text, bounds[bi]) {
            bi = fitted;
        }
        // Even then, a split that would strand closing punctuation on the
        // next line is walked back one grapheme when the remainder allows.
        if bi > si + 1 {
            while bi > si + 1 && text[bounds[bi]..].starts_with(forbidden_line_leader()) {
                bi -= 1;
            }
        }
        let end = bounds[bi];
        out.push((start, end));
        si = bi;
    }
    out
}

/// Characters that must never begin a rendered line (避头尾 line-head rule).
fn forbidden_line_leader() -> &'static [char] {
    &[
        '\u{3002}', '\u{FF0C}', '\u{3001}', '\u{FF1B}', '\u{FF1A}', '\u{FF1F}', '\u{FF01}',
        '\u{FF09}', '\u{300D}', '\u{300F}', '\u{300B}', '\u{201D}', '\u{2019}', ')', ']', '}', ',',
        '.', ';', ':', '?', '!',
    ]
}

pub(crate) struct SelectableText {
    view: Entity<WorkspaceView>,
    block: SharedString,
    text: SharedString,
    runs: Vec<SelRun>,
    font_size: Pixels,
    line_height: Pixels,
    align: TextAlign,
    wrap: bool,
}

pub(crate) struct SelectablePrepaint {
    fragments: Vec<TextFragment>,
}

impl SelectableText {
    fn to_text_runs(&self, range: Range<usize>) -> Vec<TextRun> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        for run in &self.runs {
            let run_end = pos + run.len;
            let start = pos.max(range.start);
            let end = run_end.min(range.end);
            if start < end {
                out.push(TextRun {
                    len: end - start,
                    font: run.font.clone(),
                    color: run.color,
                    background_color: run.background,
                    underline: run.underline.then(|| UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: run.strike.then(|| StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    }),
                });
            }
            pos = run_end;
        }
        out
    }

    fn layout_fragments(
        &self,
        window: &mut Window,
        wrap_width: Option<Pixels>,
    ) -> Vec<TextFragment> {
        let text = self.text.as_ref();
        if text.is_empty() {
            return Vec::new();
        }
        let text_system = window.text_system();
        let mut out = Vec::new();
        // Split on hard breaks FIRST: GPUI's `shape_line` asserts that its
        // text argument contains no newline, so a `\n` must never reach it.
        // LLM output legitimately carries newlines (soft breaks rendered as
        // line breaks, user prompts typed across several lines), and handing
        // one over aborted the whole process via
        // `text_system.rs: text argument should not contain newlines`.
        for (line_start, line_end) in hard_line_ranges(text) {
            let line_runs = self.to_text_runs(line_start..line_end);
            if line_runs.is_empty() {
                continue;
            }
            let line_text: SharedString = text[line_start..line_end].into();
            let full = text_system.shape_line(line_text.clone(), self.font_size, &line_runs, None);
            let ranges = match wrap_width {
                Some(width) => wrap_ranges(&full, &line_text, width),
                None => vec![(0, line_text.len())],
            };
            for (rel_start, rel_end) in ranges {
                let start = line_start + rel_start;
                let end = line_start + rel_end;
                let runs = self.to_text_runs(start..end);
                let fragment_text: SharedString = text[start..end].into();
                let line = text_system.shape_line(fragment_text, self.font_size, &runs, None);
                out.push(TextFragment { start, end, line });
            }
        }
        out
    }

    /// First-paint line-count estimate, computed per hard line so a
    /// multi-line block is not measured as one very wide line.
    fn estimate_line_count(&self, window: &mut Window) -> usize {
        let text = self.text.as_ref();
        if text.is_empty() {
            return 1;
        }
        let text_system = window.text_system();
        let mut total = 0usize;
        for (start, end) in hard_line_ranges(text) {
            let runs = self.to_text_runs(start..end);
            if runs.is_empty() {
                // Blank line: still occupies one row.
                total += 1;
                continue;
            }
            let line_text: SharedString = text[start..end].into();
            let shaped = text_system.shape_line(line_text, self.font_size, &runs, None);
            let width: f32 = shaped.width().into();
            total += ((width / FIRST_ESTIMATE_WIDTH).ceil() as usize).max(1);
        }
        total.max(1)
    }
}

/// Byte ranges of the hard lines in `text`, excluding the `\n` separators.
///
/// Returns one range per line; a trailing separator does not produce a
/// trailing empty range. Ranges are always `start < end` on char boundaries,
/// and never contain a `\n`, so callers can hand them straight to
/// `shape_line` and byte-range slicing.
pub(crate) fn hard_line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            if offset > start {
                out.push((start, offset));
            }
            start = offset + 1;
        }
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = SelectablePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Height comes from the cached wrap (previous frame). First paint
        // falls back to a width-guessed estimate; prepaint corrects it.
        let lines = self
            .view
            .read(cx)
            .timeline_text_layouts
            .get(self.block.as_ref())
            .map(|layout| layout.fragments.len().max(1))
            .unwrap_or_else(|| self.estimate_line_count(window));
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (self.line_height * lines as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let wrap_width = if self.wrap {
            Some(bounds.size.width)
        } else {
            None
        };
        let hash = text_hash(self.text.as_ref());
        let cached = self
            .view
            .read(cx)
            .timeline_text_layouts
            .get(self.block.as_ref())
            .cloned();
        let width_close = |a: Pixels, b: Pixels| a > b - px(1.) && a < b + px(1.);
        let fragments = if let Some(entry) = cached
            .as_ref()
            .filter(|entry| entry.text_hash == hash && width_close(entry.width, bounds.size.width))
        {
            entry.fragments.clone()
        } else {
            let fragments = self.layout_fragments(window, wrap_width);
            let links = self
                .runs
                .iter()
                .scan(0usize, |offset, run| {
                    let start = *offset;
                    *offset += run.len;
                    Some((start..*offset, run.link.clone()))
                })
                .filter_map(|(range, link)| link.map(|url| (range, url)))
                .collect();
            let width_changed = cached
                .as_ref()
                .map(|entry| entry.width != bounds.size.width)
                .unwrap_or(true);
            self.view.update(cx, |view, _| {
                view.timeline_text_layouts.insert(
                    self.block.to_string(),
                    TextBlockLayout {
                        origin: bounds.origin,
                        width: bounds.size.width,
                        line_height: self.line_height,
                        align: self.align,
                        text_hash: hash,
                        text: self.text.to_string(),
                        fragments: fragments.clone(),
                        links,
                    },
                );
            });
            if width_changed {
                // New wrap: request one more frame so `request_layout`
                // measures the final height (converges, width is stable).
                window.refresh();
            }
            self.view
                .read(cx)
                .timeline_text_layouts
                .get(self.block.as_ref())
                .cloned()
                .map(|entry| entry.fragments)
                .unwrap_or_default()
        };
        // Origin moves with scrolling independent of content: always refresh.
        self.view.update(cx, |view, _| {
            if let Some(entry) = view.timeline_text_layouts.get_mut(self.block.as_ref()) {
                entry.origin = bounds.origin;
            }
        });
        SelectablePrepaint { fragments }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let range_opt = {
            let view = self.view.read(cx);
            view.timeline_text_selection.as_ref().and_then(|sel| {
                selection_range_for_block(sel, self.block.as_ref(), &view.timeline_text_layouts)
            })
        };
        let selection = range_opt.filter(|r| !r.is_empty());
        for (index, fragment) in prepaint.fragments.iter().enumerate() {
            let line_dx = align_offset(self.align, bounds.size.width, fragment.line.width());
            let origin = point(
                bounds.origin.x + line_dx,
                bounds.origin.y + self.line_height * index as f32,
            );
            if let Some(range) = selection.as_ref() {
                let start = range.start.max(fragment.start).min(fragment.end);
                let end = range.end.max(fragment.start).min(fragment.end);
                if end > start {
                    let x0 = fragment.line.x_for_index(start - fragment.start);
                    let x1 = fragment.line.x_for_index(end - fragment.start);
                    if x1 > x0 {
                        window.paint_quad(fill(
                            Bounds::from_corners(
                                point(origin.x + x0, origin.y),
                                point(origin.x + x1, origin.y + self.line_height),
                            ),
                            selection_color(),
                        ));
                    }
                }
            }
            let _ =
                fragment
                    .line
                    .paint(origin, self.line_height, TextAlign::Left, None, window, cx);
        }
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl WorkspaceView {
    /// Container mouse-down: focus timeline text, then start/extend selection
    /// in the block under the cursor (position → block lookup).
    pub(crate) fn text_selection_down(
        &mut self,
        pos: Point<Pixels>,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.timeline_text_focus.clone().focus(window, cx);
        let block = match find_block_at(&self.timeline_text_layouts, pos) {
            Some(block) => block,
            None => return,
        };
        let layout = match self.timeline_text_layouts.get(&block) {
            Some(layout) => layout.clone(),
            None => return,
        };
        let offset = match hit_offset(&layout, &layout.text, pos) {
            Some(offset) => offset,
            None => return,
        };
        if click_count >= 3 {
            self.timeline_text_selection = Some(TimelineTextSelection {
                anchor_block: block.clone(),
                anchor_offset: 0,
                focus_block: block.clone(),
                focus_offset: layout.text.len(),
            });
            self.timeline_text_press = None;
        } else if click_count == 2 {
            let range = word_bounds_at(&layout.text, offset);
            self.timeline_text_selection = Some(TimelineTextSelection {
                anchor_block: block.clone(),
                anchor_offset: range.start,
                focus_block: block.clone(),
                focus_offset: range.end,
            });
            self.timeline_text_press = None;
        } else {
            self.timeline_text_selection = Some(TimelineTextSelection {
                anchor_block: block.clone(),
                anchor_offset: offset,
                focus_block: block.clone(),
                focus_offset: offset,
            });
            self.timeline_text_press = Some(TimelineTextPress {
                block,
                anchor: offset,
            });
        }
        cx.notify();
    }

    pub(crate) fn text_selection_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (anchor_block, anchor_offset) = match &self.timeline_text_press {
            Some(press) => (press.block.clone(), press.anchor),
            _ => return,
        };
        // Anchor must still exist; otherwise the drag has no origin.
        if !self.timeline_text_layouts.contains_key(&anchor_block) {
            return;
        }
        // Multi-block drag: the cursor may sit in gaps (padding, icons,
        // markers) or over another block. Fall back to the nearest block by
        // visual distance so crossing paragraphs/list items/tables keeps
        // extending the selection instead of stalling at the origin block.
        let focus_block = match find_block_at(&self.timeline_text_layouts, pos)
            .or_else(|| find_nearest_block(&self.timeline_text_layouts, pos))
        {
            Some(block) => block,
            None => return,
        };
        let layout = match self.timeline_text_layouts.get(&focus_block) {
            Some(layout) => layout.clone(),
            None => return,
        };
        let focus_offset = match hit_offset(&layout, &layout.text, pos) {
            Some(offset) => offset,
            None => return,
        };
        if let Some(selection) = self.timeline_text_selection.as_mut() {
            selection.anchor_block = anchor_block;
            selection.anchor_offset = anchor_offset;
            selection.focus_block = focus_block;
            selection.focus_offset = focus_offset;
        } else {
            self.timeline_text_selection = Some(TimelineTextSelection {
                anchor_block: anchor_block.clone(),
                anchor_offset,
                focus_block,
                focus_offset,
            });
        }
        cx.notify();
    }

    pub(crate) fn text_selection_up(&mut self, cx: &mut Context<Self>) {
        if self.timeline_text_press.is_some() {
            self.timeline_text_press = None;
            cx.notify();
        }
    }

    pub(crate) fn text_selection_up_out(&mut self, cx: &mut Context<Self>) {
        if self.timeline_text_press.is_some() {
            self.timeline_text_press = None;
            cx.notify();
        }
    }

    /// Row mouse-up: runs the row tap when the press stayed within threshold.
    /// Call sites pass the row id; the press must have started on the same row.
    pub(crate) fn text_row_tap(
        &mut self,
        row_id: &str,
        pos: Point<Pixels>,
        on_tap: Option<TapCallback>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press_pos = match &self.timeline_row_press {
            Some(press) if press.id == row_id => press.pos,
            _ => return,
        };
        self.timeline_row_press = None;
        let dx = pos.x - press_pos.x;
        let dy = pos.y - press_pos.y;
        if dx > px(-TAP_DRAG_THRESHOLD_PX)
            && dx < px(TAP_DRAG_THRESHOLD_PX)
            && dy > px(-TAP_DRAG_THRESHOLD_PX)
            && dy < px(TAP_DRAG_THRESHOLD_PX)
        {
            // Click (no drag): collapse any selection, then act.
            self.timeline_text_selection = None;
            // Link taps open the URL instead of the row action.
            if let Some(url) = self
                .timeline_text_layouts
                .values()
                .filter_map(|layout| {
                    if layout.origin.y <= pos.y
                        && pos.y
                            <= layout.origin.y + layout.line_height * layout.fragments.len() as f32
                        && layout.origin.x <= pos.x
                        && pos.x <= layout.origin.x + layout.width
                    {
                        hit_offset(layout, &layout.text.clone(), pos)
                            .and_then(|offset| link_at(layout, offset))
                    } else {
                        None
                    }
                })
                .next()
            {
                if is_browsable_url(&url) {
                    cx.open_url(&url);
                    cx.notify();
                    return;
                }
            }
            if let Some(tap) = on_tap {
                tap(self, window, cx);
                return;
            }
        }
        cx.notify();
    }

    pub(crate) fn text_row_press(&mut self, row_id: &str, pos: Point<Pixels>) {
        self.timeline_row_press = Some(TimelineRowPress {
            id: row_id.to_string(),
            pos,
        });
    }

    pub(crate) fn copy_timeline_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(selection) = self.timeline_text_selection.clone() {
            let text = collect_selection_text(&selection, &self.timeline_text_layouts);
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }
}

/// Byte offset in `text` for a window-space point, or `None` without layout.
fn hit_offset(layout: &TextBlockLayout, text: &str, pos: Point<Pixels>) -> Option<usize> {
    if layout.fragments.is_empty() || layout.line_height <= px(0.) {
        return None;
    }
    let rel_y: f32 = (pos.y - layout.origin.y).into();
    let line_height: f32 = layout.line_height.into();
    if line_height <= 0.0 {
        return None;
    }
    let line_index = rel_y.div_euclid(line_height) as usize;
    let fragment = if pos.y < layout.origin.y {
        layout.fragments.first()?
    } else {
        layout
            .fragments
            .get(line_index.min(layout.fragments.len() - 1))?
    };
    let frag_dx = align_offset(layout.align, layout.width, fragment.line.width());
    let local_x = (pos.x - layout.origin.x - frag_dx).max(px(0.));
    let local = fragment.line.closest_index_for_x(local_x);
    Some(snap_to_grapheme(
        text,
        (fragment.start + local).min(fragment.end),
    ))
}

fn link_at(layout: &TextBlockLayout, offset: usize) -> Option<String> {
    layout
        .links
        .iter()
        .find(|(range, _)| range.contains(&offset))
        .map(|(_, url)| url.clone())
}

pub(crate) fn is_browsable_url(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Visual position of a block for document ordering (top-to-bottom,
/// then left-to-right so table cells on one row read in column order).
fn layout_pos(layout: &TextBlockLayout) -> (f32, f32) {
    let y: f32 = layout.origin.y.into();
    let x: f32 = layout.origin.x.into();
    (y, x)
}

fn cmp_pos(a: (f32, f32), b: (f32, f32)) -> std::cmp::Ordering {
    a.0.partial_cmp(&b.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
}

/// Highlight byte range for `block` under a possibly multi-block selection.
///
/// - Same block: ordered + grapheme-snapped range (may be empty for a caret).
/// - Endpoint blocks: from/to the endpoint offset (clamped + snapped).
/// - Middle blocks: full text.
/// - Outside the anchor↔focus interval: `None`.
pub fn selection_range_for_block(
    selection: &TimelineTextSelection,
    block: &str,
    layouts: &std::collections::HashMap<String, TextBlockLayout>,
) -> Option<Range<usize>> {
    if selection.anchor_block == selection.focus_block {
        if block != selection.anchor_block {
            return None;
        }
        let layout = layouts.get(block)?;
        if layout.text.is_empty() {
            return None;
        }
        return Some(normalize_range(
            &layout.text,
            selection.anchor_offset,
            selection.focus_offset,
        ));
    }
    let anchor_layout = layouts.get(&selection.anchor_block)?;
    let focus_layout = layouts.get(&selection.focus_block)?;
    let self_layout = layouts.get(block)?;
    if self_layout.text.is_empty() {
        return None;
    }
    let anchor_pos = layout_pos(anchor_layout);
    let focus_pos = layout_pos(focus_layout);
    let self_pos = layout_pos(self_layout);
    let (start_block, start_offset, start_pos, end_block, end_offset, end_pos) =
        if cmp_pos(anchor_pos, focus_pos) <= std::cmp::Ordering::Equal {
            (
                &selection.anchor_block,
                selection.anchor_offset,
                anchor_pos,
                &selection.focus_block,
                selection.focus_offset,
                focus_pos,
            )
        } else {
            (
                &selection.focus_block,
                selection.focus_offset,
                focus_pos,
                &selection.anchor_block,
                selection.anchor_offset,
                anchor_pos,
            )
        };
    // Outside the interval: no highlight.
    if cmp_pos(self_pos, start_pos) == std::cmp::Ordering::Less
        || cmp_pos(self_pos, end_pos) == std::cmp::Ordering::Greater
    {
        return None;
    }
    let len = self_layout.text.len();
    let snap = |ix: usize| snap_to_grapheme(&self_layout.text, ix.min(len));
    if block == start_block.as_str() {
        let s = snap(start_offset);
        return Some(s..len);
    }
    if block == end_block.as_str() {
        let e = snap(end_offset);
        return Some(0..e);
    }
    Some(0..len)
}

/// Concatenate the selected slices in visual order for `Ctrl+C`.
///
/// Same-block selections behave exactly as before. Multi-block selections join
/// endpoint slices + full middle blocks; blocks sharing a visual row (table
/// cells) join with `\t`, otherwise with `\n`.
pub fn collect_selection_text(
    selection: &TimelineTextSelection,
    layouts: &std::collections::HashMap<String, TextBlockLayout>,
) -> String {
    if selection.anchor_block == selection.focus_block {
        let Some(layout) = layouts.get(&selection.anchor_block) else {
            return String::new();
        };
        let range = normalize_range(
            &layout.text,
            selection.anchor_offset,
            selection.focus_offset,
        );
        if range.is_empty() {
            return String::new();
        }
        return layout.text.get(range).unwrap_or("").to_string();
    }
    let Some(anchor_layout) = layouts.get(&selection.anchor_block) else {
        return String::new();
    };
    let Some(focus_layout) = layouts.get(&selection.focus_block) else {
        return String::new();
    };
    let anchor_pos = layout_pos(anchor_layout);
    let focus_pos = layout_pos(focus_layout);
    let (start_block, start_offset, start_pos, end_block, end_offset) =
        if cmp_pos(anchor_pos, focus_pos) <= std::cmp::Ordering::Equal {
            (
                &selection.anchor_block,
                selection.anchor_offset,
                anchor_pos,
                &selection.focus_block,
                selection.focus_offset,
            )
        } else {
            (
                &selection.focus_block,
                selection.focus_offset,
                focus_pos,
                &selection.anchor_block,
                selection.anchor_offset,
            )
        };
    let Some(end_layout) = layouts.get(end_block) else {
        return String::new();
    };
    let end_pos = layout_pos(end_layout);
    // Visual order of every laid-out block.
    let mut ordered: Vec<(&String, &TextBlockLayout)> = layouts.iter().collect();
    ordered.sort_by(|a, b| cmp_pos(layout_pos(a.1), layout_pos(b.1)));
    let start_idx = ordered.iter().position(|(k, _)| *k == start_block);
    let end_idx = ordered.iter().position(|(k, _)| *k == end_block);
    let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) else {
        return String::new();
    };
    let (lo, hi) = if start_idx <= end_idx {
        (start_idx, end_idx)
    } else {
        (end_idx, start_idx)
    };
    let mut parts: Vec<String> = Vec::new();
    let mut prev_y: Option<f32> = None;
    let mut prev_h: Option<f32> = None;
    let mut out = String::new();
    for (key, layout) in ordered[lo..=hi].iter() {
        // Skip blocks outside the Y interval (stale hidden layouts).
        let (y, _) = layout_pos(layout);
        let in_y = if start_pos.0 <= end_pos.0 {
            y >= start_pos.0 - 0.5 && y <= end_pos.0 + block_height(layout) + 0.5
        } else {
            y >= end_pos.0 - 0.5 && y <= start_pos.0 + block_height(layout) + 0.5
        };
        if !in_y {
            continue;
        }
        let len = layout.text.len();
        let slice = if key.as_str() == start_block.as_str() && key.as_str() == end_block.as_str() {
            let r = normalize_range(&layout.text, start_offset, end_offset);
            layout.text.get(r).unwrap_or("").to_string()
        } else if key.as_str() == start_block.as_str() {
            let s = snap_to_grapheme(&layout.text, start_offset.min(len));
            layout.text.get(s..).unwrap_or("").to_string()
        } else if key.as_str() == end_block.as_str() {
            let e = snap_to_grapheme(&layout.text, end_offset.min(len));
            layout.text.get(..e).unwrap_or("").to_string()
        } else {
            layout.text.clone()
        };
        if slice.is_empty() {
            continue;
        }
        // Separator: same visual row → tab (table cells), else newline.
        if !parts.is_empty() {
            let same_row = match (prev_y, prev_h) {
                (Some(py), Some(ph)) => {
                    (y - py).abs() < (ph.max(block_height(layout)) * 0.5).max(1.0)
                }
                _ => false,
            };
            out.push_str(if same_row { "\t" } else { "\n" });
        }
        out.push_str(&slice);
        parts.push(slice);
        prev_y = Some(y);
        prev_h = Some(block_height(layout));
    }
    out
}

fn block_height(layout: &TextBlockLayout) -> f32 {
    let h: f32 = layout.line_height.into();
    (h * layout.fragments.len() as f32).max(1.0)
}

/// Nearest laid-out block for drags sitting in gaps (padding, icons,
/// markers). Exact hits win; otherwise the smallest visual distance
/// (vertical primary, horizontal tie-break via Euclidean distance).
fn find_nearest_block(
    layouts: &std::collections::HashMap<String, TextBlockLayout>,
    pos: Point<Pixels>,
) -> Option<String> {
    let px: f32 = pos.x.into();
    let py: f32 = pos.y.into();
    let mut best: Option<(f32, String)> = None;
    for (key, layout) in layouts.iter() {
        if layout.fragments.is_empty() {
            continue;
        }
        let ox: f32 = layout.origin.x.into();
        let oy: f32 = layout.origin.y.into();
        let h = block_height(layout);
        let w: f32 = layout.width.into();
        let dy = if py < oy {
            oy - py
        } else if py > oy + h {
            py - (oy + h)
        } else {
            0.0
        };
        let dx = if px < ox {
            ox - px
        } else if px > ox + w {
            px - (ox + w)
        } else {
            0.0
        };
        let dist = dy * dy + dx * dx;
        match &best {
            Some((bd, _)) if dist >= *bd => {}
            _ => best = Some((dist, key.clone())),
        }
    }
    best.map(|(_, k)| k)
}

/// Block key under a window-space point (row padding included).
fn find_block_at(
    layouts: &std::collections::HashMap<String, TextBlockLayout>,
    pos: Point<Pixels>,
) -> Option<String> {
    layouts.iter().find_map(|(key, layout)| {
        if layout.fragments.is_empty() {
            return None;
        }
        let height = layout.line_height * layout.fragments.len() as f32;
        let end = point(layout.origin.x + layout.width, layout.origin.y + height);
        if pos.x >= layout.origin.x && pos.y >= layout.origin.y && pos.x <= end.x && pos.y <= end.y
        {
            Some(key.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    // NOTE: never `use super::*` here — the parent's `use gpui::*` would pull
    // gpui's `test` module into scope and break `#[test]` resolution.
    use super::{
        breakable_before, collect_selection_text, forbidden_line_leader, hard_line_ranges,
        is_break_after_punct, is_cjk, is_closing_punct, is_opening_punct, is_wide_char,
        normalize_range, selection_range_for_block, snap_to_grapheme, word_bounds_at,
        TextBlockLayout, TimelineTextSelection,
    };

    #[test]
    fn normalize_range_orders_and_snaps() {
        assert_eq!(normalize_range("hello", 3, 1), 1..3);
        assert_eq!(normalize_range("hello", 2, 2), 2..2);
        // Mid-grapheme indices snap back; out-of-range clamps.
        assert_eq!(normalize_range("h\u{e9}llo", 2, 5), 1..5);
        assert_eq!(normalize_range("hi", 0, 99), 0..2);
    }

    #[test]
    fn word_bounds_selects_ascii_runs_and_single_cjk() {
        assert_eq!(word_bounds_at("foo bar", 1), 0..3);
        assert_eq!(word_bounds_at("foo bar", 4), 4..7);
        assert_eq!(word_bounds_at("a_b-c", 2), 0..3);
        // Single CJK char (no dictionary segmentation).
        assert_eq!(word_bounds_at("你好世界", 3), 3..6);
        assert_eq!(word_bounds_at("", 0), 0..0);
    }

    #[test]
    fn word_bounds_at_end_of_multibyte_text_does_not_panic() {
        // Regression: text ending with '）' (3 bytes). Old code did
        // `snap(ix).min(len - 1)` — with `ix == len` the `min` produced a
        // mid-char index (56 inside bytes 54..57) and panicked at
        // `text[ix..]`, aborting the process from a non-unwinding callback.
        let text = "测试内容（说明）";
        let len = text.len();
        // '）' is 3 bytes; len - 1 is strictly inside it.
        assert!(!text.is_char_boundary(len - 1));
        let last_start = len - '）'.len_utf8();
        // Cursor past the end and mid-char indices must all snap safely.
        assert_eq!(word_bounds_at(text, len), last_start..len);
        assert_eq!(word_bounds_at(text, len - 1), last_start..len);
        assert_eq!(word_bounds_at(text, len + 99), last_start..len);
        assert_eq!(word_bounds_at(text, last_start + 1), last_start..len);
    }

    #[test]
    fn snap_to_grapheme_handles_combining_marks() {
        // "e" + combining acute: middle byte snaps back to grapheme start.
        let text = "cafe\u{301}";
        assert_eq!(text.len(), 6);
        assert_eq!(snap_to_grapheme(text, 4), 3);
        assert_eq!(snap_to_grapheme(text, 3), 3);
        assert_eq!(snap_to_grapheme(text, 99), text.len());
    }

    fn mock_layout(y: f32, x: f32, text: &str) -> TextBlockLayout {
        TextBlockLayout {
            origin: gpui::point(gpui::px(x), gpui::px(y)),
            width: gpui::px(400.0),
            line_height: gpui::px(18.0),
            align: gpui::TextAlign::Left,
            text_hash: 0,
            text: text.to_string(),
            fragments: Vec::new(),
            links: Vec::new(),
        }
    }

    fn three_blocks() -> std::collections::HashMap<String, TextBlockLayout> {
        let mut m = std::collections::HashMap::new();
        m.insert("a".to_string(), mock_layout(0.0, 0.0, "第一行内容"));
        m.insert("b".to_string(), mock_layout(20.0, 0.0, "第二行内容"));
        m.insert("c".to_string(), mock_layout(40.0, 0.0, "第三行内容"));
        m
    }

    #[test]
    fn multi_block_range_covers_endpoints_and_middle() {
        let layouts = three_blocks();
        // Anchor in the middle of `a`, focus in the middle of `c`.
        // "第一行内容" bytes: each CJK 3 bytes; offset 3 = after 第一.
        let sel = TimelineTextSelection {
            anchor_block: "a".to_string(),
            anchor_offset: 3,
            focus_block: "c".to_string(),
            focus_offset: 6,
        };
        assert_eq!(
            selection_range_for_block(&sel, "a", &layouts),
            Some(3.."第一行内容".len())
        );
        assert_eq!(
            selection_range_for_block(&sel, "b", &layouts),
            Some(0.."第二行内容".len())
        );
        assert_eq!(selection_range_for_block(&sel, "c", &layouts), Some(0..6));
        // Reverse drag gives the same per-block ranges.
        let rev = TimelineTextSelection {
            anchor_block: "c".to_string(),
            anchor_offset: 6,
            focus_block: "a".to_string(),
            focus_offset: 3,
        };
        assert_eq!(
            selection_range_for_block(&rev, "a", &layouts),
            Some(3.."第一行内容".len())
        );
        assert_eq!(
            selection_range_for_block(&rev, "b", &layouts),
            Some(0.."第二行内容".len())
        );
        assert_eq!(selection_range_for_block(&rev, "c", &layouts), Some(0..6));
    }

    #[test]
    fn multi_block_copy_joins_with_newlines() {
        let layouts = three_blocks();
        let sel = TimelineTextSelection {
            anchor_block: "a".to_string(),
            anchor_offset: 3,
            focus_block: "c".to_string(),
            focus_offset: 6,
        };
        let text = collect_selection_text(&sel, &layouts);
        let a_tail = "第一行内容".get(3..).unwrap();
        let c_head = "第三行内容".get(..6).unwrap();
        assert_eq!(text, format!("{a_tail}\n第二行内容\n{c_head}"));
    }

    #[test]
    fn same_block_selection_unchanged() {
        let layouts = three_blocks();
        let sel = TimelineTextSelection {
            anchor_block: "b".to_string(),
            anchor_offset: 6,
            focus_block: "b".to_string(),
            focus_offset: 3,
        };
        assert_eq!(selection_range_for_block(&sel, "b", &layouts), Some(3..6));
        assert_eq!(selection_range_for_block(&sel, "a", &layouts), None);
        assert_eq!(
            collect_selection_text(&sel, &layouts),
            "第二行内容".get(3..6).unwrap()
        );
    }

    #[test]
    fn breakable_prefers_punct_over_mid_word() {
        // `tool-registry.ts:34` must break after `-` / `.` / `:` instead of
        // mid-word — screenshot case `(tool-registry.ts:34).` went vertical
        // because no break was found and the fallback emitted 1 char/line.
        let text = "(tool-registry.ts:34).";
        // After `-`: yes.
        let dash = text.find('-').unwrap() + 1;
        assert!(breakable_before(text, dash));
        assert!(is_break_after_punct('-'));
        // Before `)` / `.`: no (closing must not start a line).
        let close = text.find(')').unwrap();
        assert!(!breakable_before(text, close));
        assert!(is_closing_punct(')'));
        assert!(is_closing_punct('.'));
        assert!(is_opening_punct('('));
        // `snake_case` / `a/b` break after `_` / `/`.
        assert!(breakable_before("a_b", 2));
        assert!(breakable_before("a/b", 2));
    }

    #[test]
    fn breakable_covers_cjk_punct_and_symbols() {
        // “” ‘’ … — → ❌ must participate in CJK breaking, not stick as
        // unbreakable ASCII islands.
        for ch in ['“', '”', '‘', '’', '…', '—', '→', '❌', '、', '。'] {
            assert!(is_cjk(ch), "expected CJK breakable: {ch}");
        }
        // Closing must not start a line; opening must not end one.
        assert!(is_closing_punct('”'));
        assert!(is_closing_punct('’'));
        assert!(is_closing_punct('…'));
        assert!(is_closing_punct('—'));
        assert!(is_closing_punct('→'));
        assert!(is_opening_punct('“'));
        assert!(is_opening_punct('‘'));
        // `过滤 → LLM`: break before `L` (after space), not before the space.
        let text = "过滤 → LLM";
        let l = text.find('L').unwrap();
        assert!(breakable_before(text, l));
    }

    #[test]
    fn forbidden_line_leader_never_starts_a_rendered_line() {
        // 避头尾: the wrap walk-back must refuse these as line leaders, which
        // is what stranded a lone `。` on its own line in the report.
        for ch in [
            '。', '，', '、', '；', '：', '？', '！', '）', '》', '」', '’', '”', ')', ',', '.',
        ] {
            assert!(
                forbidden_line_leader().contains(&ch),
                "missing forbidden line leader: {ch}"
            );
        }
        // A break before one of these is never legal.
        let text = "编译不过。";
        let at = text.find('。').unwrap();
        assert!(!breakable_before(text, at));
    }

    #[test]
    fn wide_char_detection_covers_cjk_and_emoji() {
        for ch in ['中', '。', '，', 'あ', '한', '→', '❌', '，'] {
            assert!(is_wide_char(ch), "expected wide: {ch}");
        }
        for ch in ['a', 'Z', '1', ' ', '-', '.'] {
            assert!(!is_wide_char(ch), "expected narrow: {ch}");
        }
    }

    /// Regression for the hard process abort:
    /// `panic: text argument should not contain newlines` (gpui
    /// `text_system.rs`), exit code `0xC0000409`. A `\n` reached `shape_line`
    /// from a blockquote with a soft break and from the user message card.
    #[test]
    fn hard_line_ranges_never_yield_a_newline() {
        for text in [
            "a\nb",
            "第一行\n第二行",
            "a\n\nb",
            "trailing\n",
            "\nleading",
            "a\nb\nc",
            "",
            "\n",
        ] {
            for (start, end) in hard_line_ranges(text) {
                assert!(start < end, "empty range in {text:?}: {start}..{end}");
                assert!(
                    !text[start..end].contains('\n'),
                    "range {start}..{end} of {text:?} contains a newline"
                );
                assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
            }
        }
    }

    #[test]
    fn hard_line_ranges_split_on_every_breaker() {
        assert_eq!(hard_line_ranges("a\nb"), vec![(0, 1), (2, 3)]);
        assert_eq!(hard_line_ranges("a\n\nb"), vec![(0, 1), (3, 4)]);
        assert_eq!(hard_line_ranges("a\n"), vec![(0, 1)]);
        assert_eq!(hard_line_ranges("\na"), vec![(1, 2)]);
        assert_eq!(hard_line_ranges("abc"), vec![(0, 3)]);
        assert!(hard_line_ranges("").is_empty());
        // Multibyte lines must land on char boundaries.
        let text = "中文\n测试";
        for (start, end) in hard_line_ranges(text) {
            assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        }
        assert_eq!(hard_line_ranges(text), vec![(0, 6), (7, 13)]);
    }
}
