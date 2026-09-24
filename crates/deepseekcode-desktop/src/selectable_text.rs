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
    pub block: String,
    pub text: String,
    pub range: Range<usize>,
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

fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{FF00}'..='\u{FFEF}'
        | '\u{3000}'..='\u{303F}')
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
            | '%'
            | '‰'
            | '°'
    )
}

/// Opening punctuation that must not end a wrapped line.
fn is_opening_punct(ch: char) -> bool {
    matches!(
        ch,
        '（' | '《' | '「' | '『' | '〈' | '(' | '[' | '{' | '<' | '“' | '‘' | '￥' | '$'
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
    let ix = snap_to_grapheme(text, ix).min(text.len().saturating_sub(1).max(0));
    let ch = text[ix..].chars().next();
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
            if p.is_whitespace() {
                return true;
            }
            if is_closing_punct(n) || is_opening_punct(p) {
                return false;
            }
            is_cjk(p) || is_cjk(n)
        }
        _ => false,
    }
}

/// Greedy wrap of one shaped source line into byte ranges (grapheme-safe).
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
        // Snap back to a break opportunity; force-split when hopeless
        // (overlong token) to guarantee progress.
        let mut bi = best.max(si + 1);
        while bi > si + 1 && !breakable_before(text, bounds[bi]) {
            bi -= 1;
        }
        let end = bounds[bi];
        out.push((start, end));
        si = bi;
    }
    out
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
        let full_runs = self.to_text_runs(0..text.len());
        if full_runs.is_empty() {
            return Vec::new();
        }
        let full = text_system.shape_line(self.text.clone(), self.font_size, &full_runs, None);
        let ranges = match wrap_width {
            Some(width) => wrap_ranges(&full, text, width),
            None => vec![(0, text.len())],
        };
        ranges
            .into_iter()
            .map(|(start, end)| {
                let runs = self.to_text_runs(start..end);
                let fragment_text: SharedString = text[start..end].into();
                let line = text_system.shape_line(fragment_text, self.font_size, &runs, None);
                TextFragment { start, end, line }
            })
            .collect()
    }
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
            .unwrap_or_else(|| {
                let full = window.text_system().shape_line(
                    self.text.clone(),
                    self.font_size,
                    &self.to_text_runs(0..self.text.len()),
                    None,
                );
                let width: f32 = full.width().into();
                ((width / FIRST_ESTIMATE_WIDTH).ceil() as usize).max(1)
            });
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
        let reuse = cached.as_ref().is_some_and(|entry| {
            entry.text_hash == hash && width_close(entry.width, bounds.size.width)
        });
        let fragments = if reuse {
            cached.expect("checked above").fragments
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
        let selection = self
            .view
            .read(cx)
            .timeline_text_selection
            .clone()
            .filter(|sel| sel.block == self.block.as_ref() && !sel.range.is_empty());
        for (index, fragment) in prepaint.fragments.iter().enumerate() {
            let line_dx = align_offset(self.align, bounds.size.width, fragment.line.width());
            let origin = point(
                bounds.origin.x + line_dx,
                bounds.origin.y + self.line_height * index as f32,
            );
            if let Some(sel) = selection.as_ref() {
                let start = sel.range.start.max(fragment.start).min(fragment.end);
                let end = sel.range.end.max(fragment.start).min(fragment.end);
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
                block: block.clone(),
                text: layout.text.clone(),
                range: 0..layout.text.len(),
            });
            self.timeline_text_press = None;
        } else if click_count == 2 {
            let range = word_bounds_at(&layout.text, offset);
            self.timeline_text_selection = Some(TimelineTextSelection {
                block: block.clone(),
                text: layout.text.clone(),
                range,
            });
            self.timeline_text_press = None;
        } else {
            self.timeline_text_selection = Some(TimelineTextSelection {
                block: block.clone(),
                text: layout.text.clone(),
                range: offset..offset,
            });
            self.timeline_text_press = Some(TimelineTextPress {
                block,
                anchor: offset,
            });
        }
        cx.notify();
    }

    pub(crate) fn text_selection_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (block, anchor) = match &self.timeline_text_press {
            Some(press) => (press.block.clone(), press.anchor),
            _ => return,
        };
        let layout = match self.timeline_text_layouts.get(&block) {
            Some(layout) => layout.clone(),
            None => return,
        };
        let offset = match hit_offset(&layout, &layout.text, pos) {
            Some(offset) => offset,
            None => return,
        };
        if let Some(selection) = self.timeline_text_selection.as_mut() {
            if selection.block == block {
                selection.range = normalize_range(&layout.text, anchor, offset);
            }
        }
        cx.notify();
    }

    pub(crate) fn text_selection_up(&mut self, cx: &mut Context<Self>) {
        let press = match self.timeline_text_press.clone() {
            Some(press) => press,
            None => return,
        };
        self.timeline_text_press = None;
        let layout = match self.timeline_text_layouts.get(&press.block) {
            Some(layout) => layout.clone(),
            None => return,
        };
        // Refresh the snapshot text so copy uses current content.
        if let Some(selection) = self.timeline_text_selection.as_mut() {
            if selection.block == press.block {
                selection.text = layout.text.clone();
            }
        }
        cx.notify();
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
        if let Some(selection) = self.timeline_text_selection.as_ref() {
            let range =
                normalize_range(&selection.text, selection.range.start, selection.range.end);
            if !range.is_empty() {
                if let Some(text) = selection.text.get(range) {
                    if !text.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                    }
                }
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
    use super::{normalize_range, snap_to_grapheme, word_bounds_at};

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
    fn snap_to_grapheme_handles_combining_marks() {
        // "e" + combining acute: middle byte snaps back to grapheme start.
        let text = "cafe\u{301}";
        assert_eq!(text.len(), 6);
        assert_eq!(snap_to_grapheme(text, 4), 3);
        assert_eq!(snap_to_grapheme(text, 3), 3);
        assert_eq!(snap_to_grapheme(text, 99), text.len());
    }
}
