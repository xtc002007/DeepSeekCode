use crate::selectable_text::{selectable_text, SelectableStyle, SelectableText};
use crate::theme::ThemeTokens;
use crate::workspace::WorkspaceView;
use gpui::*;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Render markdown source into GPUI elements.
///
/// `salt` scopes scroll-container element ids (code blocks, tables) so
/// repeated renders (one per timeline message) never collide. Pass a stable
/// per-message key (leaf id / detail key) to keep scroll positions stable.
/// `view` backs drag-selectable text blocks.
pub fn render_markdown(
    source: &str,
    theme: &ThemeTokens,
    salt: &str,
    view: &Entity<WorkspaceView>,
) -> impl IntoElement {
    let parsed = parse_blocks(source);
    let blocks = BlockRenderer::new(theme, salt, view).render(parsed);
    let leading = crate::theme::leading_relaxed(theme);
    let mut container = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .line_height(leading);
    for (index, block) in blocks.into_iter().enumerate() {
        match block {
            // Headings breathe: top padding unless first block.
            Block::Heading(el) if index > 0 => {
                container = container.child(div().w_full().flex().flex_col().pt_2().child(el));
            }
            Block::Heading(el) | Block::Body(el) => {
                container = container.child(el);
            }
        }
    }
    container
}

/// Top-level block with heading flag (headings get extra top spacing).
enum Block {
    Heading(AnyElement),
    Body(AnyElement),
}

/// Pure parse: markdown source → data tree (no theme, view, or elements).
/// Fully unit-testable, including block content.
fn parse_blocks(source: &str) -> Vec<ParsedBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    // Re-join the model's own hard wrapping BEFORE parsing, so the wrap
    // points chosen by the provider never survive into the UI.
    let reflowed = reflow_hard_wrapped(source);
    let parser = Parser::new_ext(&reflowed, options);
    let mut ctx = ParseContext::new();

    for event in parser {
        ctx.handle(event);
    }
    ctx.finish()
}

/// Column width above which a line break is treated as a model hard-wrap
/// artifact rather than authored structure. Measured in display columns
/// (CJK = 2) so a 20-char Chinese line and a 40-char English line carry the
/// same "this was column-wrapped" signal. Short lines (`1.`, `-`, a short
/// authored sentence) always stand alone.
const WRAP_ARTIFACT_MIN_COLUMNS: usize = 40;

/// Display width in "columns" (CJK/fullwidth = 2, everything else = 1).
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if crate::selectable_text::is_wide_char(ch) {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Fence delimiter (```` ``` ```` / `~~~`) if the line opens or closes one.
fn fence_marker(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    for ch in ['`', '~'] {
        let count = trimmed.chars().take_while(|c| *c == ch).count();
        if count >= 3 {
            return Some(ch.to_string().repeat(count));
        }
    }
    None
}

/// Does this line start a new block (list, heading, quote, table, fence)?
///
/// Used to protect authored structure from reflow: `- item`, `1. item`,
/// `> quote`, `| a | b |`, `# title`, `---` and fences always stand alone.
fn starts_block(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    if fence_marker(trimmed).is_some() {
        return true;
    }
    if trimmed.starts_with('#') {
        return true;
    }
    if trimmed.starts_with('>') {
        return true;
    }
    if trimmed.starts_with('|') {
        return true;
    }
    // Ordered / bullet list markers (also task lists `- [ ]`).
    let bullet = trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("-\t")
        || trimmed.starts_with("*\t")
        || trimmed.starts_with("+\t");
    if bullet {
        return true;
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if (rest.starts_with('.') || rest.starts_with(')')) && rest[1..].starts_with([' ', '\t']) {
            return true;
        }
    }
    // Thematic break / setext underline.
    if trimmed.chars().all(|c| matches!(c, '-' | '=' | '*' | '_')) && trimmed.len() >= 3 {
        return true;
    }
    false
}

/// Re-join provider hard wrapping in LLM output.
///
/// Models emit long paragraphs already wrapped at a fixed column. CommonMark
/// reads a single `\n` as a soft break, but chat surfaces must also treat it
/// as a hard break (LLM output uses `\n` for real line structure) — so the
/// model's arbitrary column boundaries would otherwise reach the screen and
/// split words mid-token, producing orphan lines such as `过` / `。`.
///
/// Rules (all conservative — a wrong join loses a line break, a wrong split
/// shows a broken word):
/// - never touch fenced code, tables, or 4-space indented code;
/// - never join a line that starts a new block;
/// - never join after an explicit hard break (two trailing spaces or `\\`);
/// - never join when the next line is a continuation marker (`*`, `_`, `|`) or
///   an HTML tag — inline markdown would break across the join point;
/// - only join a break whose previous line is long enough to have been
///   column-wrapped.
fn reflow_hard_wrapped(source: &str) -> String {
    if !source.contains('\n') {
        return source.to_string();
    }
    // Cheap reject: a document with no long lines is already structured.
    let had_crlf = source.contains("\r\n");
    let normalized = if had_crlf {
        source.replace("\r\n", "\n")
    } else {
        source.to_string()
    };
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<String> = None;
    let mut in_table = false;
    for line in lines {
        let trimmed = line.trim();
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();

        // --- fenced code: byte-exact, never reflowed -------------------
        if let Some(marker) = fence_marker(trimmed_start) {
            match &fence {
                Some(open) if open.starts_with(&marker) => fence = None,
                Some(_) => {}
                None => fence = Some(marker),
            }
            out.push(line.to_string());
            continue;
        }
        if fence.is_some() || indent >= 4 {
            out.push(line.to_string());
            continue;
        }

        // --- table rows: structure, not prose ---------------------------
        let is_table_row = trimmed_start.starts_with('|');
        if is_table_row {
            in_table = true;
        } else if in_table && !trimmed.is_empty() {
            in_table = false;
        }
        if in_table {
            out.push(line.to_string());
            continue;
        }

        // Blank line ends the current paragraph: flush and keep the break.
        if trimmed.is_empty() {
            out.push(line.to_string());
            continue;
        }

        // Authored structure never joins.
        if starts_block(line) {
            out.push(line.to_string());
            continue;
        }

        // Try to re-join with the previous output line.
        let joined = out.last().and_then(|prev| {
            // Explicit hard break (Markdown: two+ trailing spaces, or `\`).
            if prev.ends_with("  ") || prev.ends_with('\\') {
                return None;
            }
            let prev_trim_end = prev.trim_end();
            if prev_trim_end.is_empty() {
                return None;
            }
            // The wrap point was real only if the previous line was long
            // enough to have been column-wrapped.
            if display_width(prev_trim_end) < WRAP_ARTIFACT_MIN_COLUMNS {
                return None;
            }
            // A leading inline-continuation marker means the author wrapped
            // deliberately so the marker keeps its content on the same line.
            if starts_inline_continuation(trimmed) {
                return None;
            }
            // Joining across an inline-code/emphasis boundary would strand
            // the closing marker on a new line.
            if has_dangling_inline_marker(prev_trim_end) || has_dangling_inline_marker(trimmed) {
                return None;
            }
            Some(join_wrapped_lines(prev_trim_end, trimmed))
        });
        match joined {
            Some(merged) => {
                let n = out.len();
                out[n - 1] = merged;
            }
            None => out.push(line.to_string()),
        }
    }
    let mut result = out.join("\n");
    if had_crlf {
        result = result.replace('\n', "\r\n");
    }
    result
}

/// Join two prose lines, inserting a space only when both sides are Latin.
///
/// `很可能编译不` + `过` must become `很可能编译不过` (no space in CJK);
/// `only produce valid` + `output.` must keep one space.
fn join_wrapped_lines(left: &str, right: &str) -> String {
    let left_last = left.chars().next_back();
    let right_first = right.chars().next();
    let need_space = match (left_last, right_first) {
        (Some(l), Some(r)) => {
            let l_wide = crate::selectable_text::is_wide_char(l);
            let r_wide = crate::selectable_text::is_wide_char(r);
            // Mixed CJK/Latin adjacency reads better with a space.
            !(l_wide && r_wide)
        }
        _ => false,
    };
    if need_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

/// A leading `*`, `_`, `|`, or HTML tag: the author broke here on purpose so
/// the marker keeps its content on the same line.
fn starts_inline_continuation(trimmed: &str) -> bool {
    if trimmed.starts_with('|') {
        return true;
    }
    if trimmed.starts_with('>') || trimmed.starts_with('#') {
        return true;
    }
    trimmed.starts_with('<')
}

/// An inline marker left dangling at the end of a line (`**粗体` / `` `code ``
/// / `*em`), i.e. the emphasis/code span continues on the next line.
///
/// Counts *delimiter runs* (maximal same-character sequences), not raw
/// characters: `**` is one run, so `**粗` (one run, unclosed) is dangling while
/// `**bold**` (two runs, closed) is not. Intraword markers such as the `_` in
/// `MAX_HISTORY_MESSAGES` are not delimiters in CommonMark and are ignored, so
/// snake_case identifiers never block re-flow.
fn has_dangling_inline_marker(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    // Count qualifying delimiter runs per marker character.
    for marker in ['`', '*', '_'] {
        let mut runs = 0usize;
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] != marker {
                i += 1;
                continue;
            }
            let run_start = i;
            while i < chars.len() && chars[i] == marker {
                i += 1;
            }
            let before = if run_start == 0 {
                None
            } else {
                Some(chars[run_start - 1])
            };
            let after = chars.get(i).copied();
            let intraword = matches!(before, Some(b) if is_word_byte(b))
                && matches!(after, Some(a) if is_word_byte(a));
            if intraword {
                continue;
            }
            // A run of one `*` is a list bullet, not emphasis.
            if marker == '*' && i - run_start == 1 && before.is_none() {
                continue;
            }
            runs += 1;
        }
        if runs % 2 == 1 {
            return true;
        }
    }
    false
}

/// Word byte for intraword-delimiter detection (ASCII letters/digits/`_`).
fn is_word_byte(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Parsed markdown tree. Every variant carries raw runs/text; rendering
/// (which needs theme + view) happens in [`BlockRenderer`].
#[derive(Clone, Debug)]
pub(crate) enum ParsedBlock {
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    Paragraph(Vec<InlineRun>),
    Code {
        lang: String,
        source: String,
    },
    List {
        items: Vec<Vec<ParsedBlock>>,
        ordered: bool,
        start: u64,
    },
    Quote(Vec<ParsedBlock>),
    Table {
        header: Vec<Vec<InlineRun>>,
        rows: Vec<Vec<Vec<InlineRun>>>,
        alignments: Vec<Alignment>,
    },
    Rule,
}

/// One nesting container: lists and quotes stack so nested structures keep
/// their parent instead of leaking to the top level.
enum Container {
    List(ListFrame),
    Quote(QuoteFrame),
}

struct ListFrame {
    ordered: bool,
    start: u64,
    items: Vec<Vec<ParsedBlock>>,
    current: Vec<ParsedBlock>,
}

struct QuoteFrame {
    blocks: Vec<ParsedBlock>,
}

/// Alt-text capture for `![alt](url)` (issue #7).
struct ImageCapture {
    url: String,
    alt: String,
}

/// Pure parse state: accumulators only, no theme/view/elements.
struct ParseContext {
    blocks: Vec<ParsedBlock>,
    inlines: Vec<InlineRun>,
    inline_stack: Vec<InlineStyle>,
    link_stack: Vec<String>,
    image_stack: Vec<ImageCapture>,
    heading_level: Option<u8>,
    in_code_block: bool,
    code_lang: String,
    code_lines: String,
    container_stack: Vec<Container>,
    in_html_block: bool,
    html_lines: String,
    table_header: Vec<Vec<InlineRun>>,
    table_rows: Vec<Vec<Vec<InlineRun>>>,
    table_row: Vec<Vec<InlineRun>>,
    table_cell: Vec<InlineRun>,
    in_table: bool,
    in_table_head: bool,
    table_alignments: Vec<Alignment>,
}

impl ParseContext {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            inlines: Vec::new(),
            inline_stack: Vec::new(),
            link_stack: Vec::new(),
            image_stack: Vec::new(),
            heading_level: None,
            in_code_block: false,
            code_lang: String::new(),
            code_lines: String::new(),
            container_stack: Vec::new(),
            in_html_block: false,
            html_lines: String::new(),
            table_header: Vec::new(),
            table_rows: Vec::new(),
            table_row: Vec::new(),
            table_cell: Vec::new(),
            in_table: false,
            in_table_head: false,
            table_alignments: Vec::new(),
        }
    }

    /// Deliver a finished block to the innermost container (or top level).
    fn emit_block(&mut self, block: ParsedBlock) {
        match self.container_stack.last_mut() {
            Some(Container::List(frame)) => frame.current.push(block),
            Some(Container::Quote(frame)) => frame.blocks.push(block),
            None => self.blocks.push(block),
        }
    }

    fn flush_inlines(&mut self) {
        if self.inlines.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.inlines);
        if let Some(level) = self.heading_level.take() {
            self.emit_block(ParsedBlock::Heading { level, runs });
            return;
        }
        self.emit_block(ParsedBlock::Paragraph(runs));
    }

    fn push_text(&mut self, text: &str) {
        // Image alt capture (issue #7): alt text must not leak into the flow.
        if let Some(capture) = self.image_stack.last_mut() {
            capture.alt.push_str(text);
            return;
        }
        if self.in_code_block {
            self.code_lines.push_str(text);
            return;
        }
        if self.in_table {
            let styles = self.active_styles();
            let url = self.link_stack.last().cloned();
            push_run_into(&mut self.table_cell, styles, url, text);
            return;
        }
        let styles = self.active_styles();
        let url = self.link_stack.last().cloned();
        push_run_into(&mut self.inlines, styles, url, text);
    }

    /// Current nesting as a run style set (empty stack = explicit Normal so
    /// every run carries a complete, comparable style).
    fn active_styles(&self) -> Vec<InlineStyle> {
        if self.inline_stack.is_empty() {
            vec![InlineStyle::Normal]
        } else {
            self.inline_stack.clone()
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    self.flush_inlines();
                    self.heading_level = Some(level as u8);
                }
                Tag::Paragraph => self.flush_inlines(),
                Tag::List(start) => {
                    self.flush_inlines();
                    self.container_stack.push(Container::List(ListFrame {
                        ordered: start.is_some(),
                        start: start.unwrap_or(1),
                        items: Vec::new(),
                        current: Vec::new(),
                    }));
                }
                Tag::Item => {
                    // Current item was sealed at the previous `TagEnd::Item`;
                    // just flush strays (balanced parsers leave none).
                    self.flush_inlines();
                }
                Tag::CodeBlock(kind) => {
                    self.flush_inlines();
                    self.in_code_block = true;
                    self.code_lines.clear();
                    self.code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                }
                Tag::BlockQuote(_) => {
                    self.flush_inlines();
                    self.container_stack
                        .push(Container::Quote(QuoteFrame { blocks: Vec::new() }));
                }
                Tag::HtmlBlock => {
                    self.flush_inlines();
                    self.in_html_block = true;
                    self.html_lines.clear();
                }
                Tag::Table(alignments) => {
                    self.flush_inlines();
                    self.in_table = true;
                    self.in_table_head = false;
                    self.table_header.clear();
                    self.table_rows.clear();
                    self.table_row.clear();
                    self.table_cell.clear();
                    self.table_alignments = alignments;
                }
                Tag::TableHead => self.in_table_head = true,
                Tag::TableRow => {
                    self.table_row.clear();
                    self.table_cell.clear();
                }
                Tag::TableCell => self.table_cell.clear(),
                Tag::Emphasis => self.inline_stack.push(InlineStyle::Emphasis),
                Tag::Strong => self.inline_stack.push(InlineStyle::Strong),
                Tag::Strikethrough => self.inline_stack.push(InlineStyle::Strike),
                Tag::Link { dest_url, .. } => {
                    self.inline_stack.push(InlineStyle::Link);
                    self.link_stack.push(dest_url.to_string());
                }
                Tag::Image { dest_url, .. } => {
                    self.flush_inlines();
                    self.image_stack.push(ImageCapture {
                        url: dest_url.to_string(),
                        alt: String::new(),
                    });
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(level) => {
                    if self.heading_level == Some(level as u8) {
                        self.flush_inlines();
                    }
                }
                TagEnd::Paragraph => self.flush_inlines(),
                TagEnd::Item => {
                    self.flush_inlines();
                    if let Some(Container::List(frame)) = self.container_stack.last_mut() {
                        frame.items.push(std::mem::take(&mut frame.current));
                    }
                }
                TagEnd::List(_) => {
                    self.flush_inlines();
                    if let Some(Container::List(frame)) = self.container_stack.pop() {
                        self.emit_block(ParsedBlock::List {
                            items: frame.items,
                            ordered: frame.ordered,
                            start: frame.start,
                        });
                    }
                }
                TagEnd::CodeBlock => {
                    self.in_code_block = false;
                    let lang = std::mem::take(&mut self.code_lang);
                    let source = std::mem::take(&mut self.code_lines);
                    self.emit_block(ParsedBlock::Code { lang, source });
                }
                TagEnd::BlockQuote(_) => {
                    self.flush_inlines();
                    if let Some(Container::Quote(frame)) = self.container_stack.pop() {
                        self.emit_block(ParsedBlock::Quote(frame.blocks));
                    }
                }
                TagEnd::HtmlBlock => {
                    self.in_html_block = false;
                    let text = strip_html_tags(&self.html_lines).trim().to_string();
                    self.html_lines.clear();
                    if !text.is_empty() {
                        let runs = vec![InlineRun {
                            styles: Vec::new(),
                            text,
                            url: None,
                        }];
                        self.emit_block(ParsedBlock::Paragraph(runs));
                    }
                }
                TagEnd::Table => {
                    self.in_table = false;
                    let header = std::mem::take(&mut self.table_header);
                    let rows = std::mem::take(&mut self.table_rows);
                    let alignments = std::mem::take(&mut self.table_alignments);
                    self.emit_block(ParsedBlock::Table {
                        header,
                        rows,
                        alignments,
                    });
                }
                TagEnd::TableHead => self.in_table_head = false,
                TagEnd::TableRow => {
                    if self.in_table_head {
                        self.table_header = self.table_row.clone();
                    } else {
                        self.table_rows.push(self.table_row.clone());
                    }
                    self.table_row.clear();
                }
                TagEnd::TableCell => {
                    trim_cell_runs(&mut self.table_cell);
                    self.table_row.push(std::mem::take(&mut self.table_cell));
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    self.inline_stack.pop();
                }
                TagEnd::Link => {
                    self.inline_stack.pop();
                    self.link_stack.pop();
                }
                TagEnd::Image => {
                    if let Some(capture) = self.image_stack.pop() {
                        let text = if capture.alt.trim().is_empty() {
                            capture.url.clone()
                        } else {
                            format!("🖼 {}", capture.alt.trim())
                        };
                        // Route like normal text: images inside table cells
                        // must stay in the cell, not escape to `inlines`.
                        if self.in_table {
                            push_run_into(
                                &mut self.table_cell,
                                vec![InlineStyle::Image],
                                Some(capture.url),
                                &text,
                            );
                        } else {
                            push_run_into(
                                &mut self.inlines,
                                vec![InlineStyle::Image],
                                Some(capture.url),
                                &text,
                            );
                        }
                    }
                }
                _ => {}
            },
            Event::Text(text) => self.push_text(&text),
            Event::Code(text) => {
                if let Some(capture) = self.image_stack.last_mut() {
                    capture.alt.push_str(&text);
                    return;
                }
                self.inline_stack.push(InlineStyle::Code);
                self.push_text(&text);
                self.inline_stack.pop();
            }
            Event::SoftBreak => {
                if let Some(capture) = self.image_stack.last_mut() {
                    capture.alt.push(' ');
                } else {
                    // 聊天输出惯例：单个换行即分行。Markdown 语义是空格，
                    // 但 LLM 中文输出常用单 `\n` 分段；若按空格合并，
                    // 长段落挤成一大段，既难读又放大换行压力。
                    self.push_text("\n");
                }
            }
            Event::HardBreak => {
                if let Some(capture) = self.image_stack.last_mut() {
                    capture.alt.push(' ');
                } else {
                    self.push_text("\n");
                }
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                if self.in_html_block {
                    self.html_lines.push_str(&text);
                    self.html_lines.push('\n');
                } else if is_line_break_html(&text) {
                    if let Some(capture) = self.image_stack.last_mut() {
                        capture.alt.push(' ');
                    } else {
                        // `<br/>` becomes a hard break (split into lines).
                        self.push_text("\n");
                    }
                }
                // Other inline tags (`<kbd>`, …) carry no text of their own.
            }
            Event::Rule => {
                self.flush_inlines();
                self.emit_block(ParsedBlock::Rule);
            }
            Event::TaskListMarker(checked) => {
                self.push_text(if checked { "☑ " } else { "☐ " });
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<ParsedBlock> {
        self.flush_inlines();
        self.blocks
    }
}

/// `<br>`, `<br/>`, `<br />` (any case). Byte-safe: never slices into a
/// multibyte char boundary.
fn is_line_break_html(html: &str) -> bool {
    let trimmed = html.trim_start();
    trimmed
        .get(..3)
        .map(|head| head.eq_ignore_ascii_case("<br"))
        .unwrap_or(false)
}

/// Tag stripper for HTML-block fallback: drops `<…>` spans. A bare `<` not
/// followed by a tag-start char (e.g. `a < b`) is kept as literal text.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' && matches!(chars.peek(), Some('a'..='z' | 'A'..='Z' | '/' | '!' | '?')) {
            for ch in chars.by_ref() {
                if ch == '>' {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InlineStyle {
    Normal,
    Strong,
    Emphasis,
    Code,
    Link,
    Strike,
    Image,
}

#[derive(Clone, Debug)]
pub(crate) struct InlineRun {
    /// Full nesting stack (issue #13: `***x***` keeps both styles).
    pub(crate) styles: Vec<InlineStyle>,
    pub(crate) text: String,
    /// Link/image destination (issue #9: clickable when browsable).
    pub(crate) url: Option<String>,
}

/// Append text, merging into the previous run when style+url match.
fn push_run_into(
    target: &mut Vec<InlineRun>,
    styles: Vec<InlineStyle>,
    url: Option<String>,
    text: &str,
) {
    if let Some(last) = target.last_mut() {
        if last.styles == styles && last.url == url {
            last.text.push_str(text);
            return;
        }
    }
    target.push(InlineRun {
        styles,
        text: text.to_string(),
        url,
    });
}

/// Trim edge whitespace of a table cell's run list.
fn trim_cell_runs(cell: &mut Vec<InlineRun>) {
    while cell.len() > 1 && cell[0].text.trim_start().is_empty() {
        cell.remove(0);
    }
    if let Some(first) = cell.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    while cell.len() > 1 && cell[cell.len() - 1].text.trim_end().is_empty() {
        cell.pop();
    }
    if let Some(last) = cell.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
}

/// Render pass: parsed data tree → elements (needs theme + view).
struct BlockRenderer<'a> {
    theme: &'a ThemeTokens,
    view: Entity<WorkspaceView>,
    salt: String,
    scroll_seq: usize,
    text_seq: usize,
}

impl<'a> BlockRenderer<'a> {
    fn new(theme: &'a ThemeTokens, salt: &str, view: &Entity<WorkspaceView>) -> Self {
        Self {
            theme,
            view: view.clone(),
            salt: salt.to_string(),
            scroll_seq: 0,
            text_seq: 0,
        }
    }

    /// Unique scroll-container id, stable per message (`salt`).
    fn next_scroll_id(&mut self, kind: &str) -> String {
        let id = format!("md-{}-{kind}-{}", self.salt, self.scroll_seq);
        self.scroll_seq += 1;
        id
    }

    /// Stable per-block key for selectable text (`salt` + document order).
    fn next_text_key(&mut self) -> String {
        let key = format!("{}#t{}", self.salt, self.text_seq);
        self.text_seq += 1;
        key
    }

    fn ui_family(&self) -> SharedString {
        crate::theme::font_ui(self.theme)
    }

    fn body_style(&self) -> SelectableStyle {
        SelectableStyle {
            size: self.theme.typography.text_xs,
            line_height: crate::theme::leading_relaxed(self.theme),
            color: self.theme.markdown.text,
            weight: FontWeight::NORMAL,
            italic: false,
            family: self.ui_family(),
            align: TextAlign::Left,
        }
    }

    /// Drag-selectable inline line (quote lines, table cells).
    fn inline_text(
        &mut self,
        runs: Vec<InlineRun>,
        style: SelectableStyle,
        wrap: bool,
    ) -> SelectableText {
        let key = self.next_text_key();
        selectable_text(&self.view, key, &runs, self.theme, &style, wrap)
    }

    fn render(&mut self, blocks: Vec<ParsedBlock>) -> Vec<Block> {
        blocks
            .into_iter()
            .map(|block| match block {
                ParsedBlock::Heading { level, runs } => {
                    Block::Heading(self.render_heading(level, runs))
                }
                other => Block::Body(self.render_parsed(other)),
            })
            .collect()
    }

    fn render_parsed(&mut self, block: ParsedBlock) -> AnyElement {
        match block {
            ParsedBlock::Heading { level, runs } => self.render_heading(level, runs),
            ParsedBlock::Paragraph(runs) => self.render_paragraph(runs),
            ParsedBlock::Code { lang, source } => self.render_code(&lang, &source),
            ParsedBlock::List {
                items,
                ordered,
                start,
            } => self.render_list(items, ordered, start),
            ParsedBlock::Quote(blocks) => self.render_quote(blocks),
            ParsedBlock::Table {
                header,
                rows,
                alignments,
            } => self.render_table(&header, &rows, &alignments),
            ParsedBlock::Rule => div()
                .w_full()
                .h(px(1.))
                .bg(self.theme.markdown.border)
                .into_any_element(),
        }
    }

    fn heading_style(&self, level: u8) -> (SelectableStyle, bool) {
        let md = &self.theme.markdown;
        let typo = &self.theme.typography;
        let size = match level {
            1 => typo.text_lg,
            2 => typo.text_base,
            _ => typo.text_sm,
        };
        (
            SelectableStyle {
                size,
                line_height: crate::theme::leading_relaxed(self.theme),
                color: md.text,
                weight: FontWeight::BOLD,
                italic: false,
                family: crate::theme::font_ui(self.theme),
                align: TextAlign::Left,
            },
            level == 1,
        )
    }

    fn render_heading(&mut self, level: u8, runs: Vec<InlineRun>) -> AnyElement {
        let md_border: Rgba = self.theme.markdown.border;
        let (style, underline) = self.heading_style(level);
        let mut el = div().w_full().cursor(CursorStyle::IBeam);
        if underline {
            el = el.border_b_1().border_color(md_border).pb_1();
        }
        // Issue #1: one selectable element wraps all runs, so mixed styles
        // (text + code + link) stay on one logical line and stay selectable.
        let key = self.next_text_key();
        el.child(selectable_text(
            &self.view, key, &runs, self.theme, &style, true,
        ))
        .into_any_element()
    }

    fn render_paragraph(&mut self, runs: Vec<InlineRun>) -> AnyElement {
        let mut column = div().w_full().flex().flex_col().gap_0p5();
        for line_runs in split_runs_on_newlines(runs) {
            let body = self.body_style();
            let el = self.inline_text(line_runs, body, true);
            column = column.child(div().cursor(CursorStyle::IBeam).child(el));
        }
        column.into_any_element()
    }
}

/// Split inline runs at hard line breaks while preserving style per segment.
fn split_runs_on_newlines(runs: Vec<InlineRun>) -> Vec<Vec<InlineRun>> {
    let mut lines: Vec<Vec<InlineRun>> = vec![Vec::new()];
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        for (index, segment) in run.text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            if segment.is_empty() {
                continue;
            }
            // `lines` starts with one empty vec and only grows, but UI code
            // must not crash on a future refactor — skip the segment instead.
            if let Some(line) = lines.last_mut() {
                line.push(InlineRun {
                    text: segment.to_string(),
                    styles: run.styles.clone(),
                    url: run.url.clone(),
                });
            }
        }
    }
    if lines.len() == 1 && lines[0].is_empty() {
        return vec![];
    }
    lines.retain(|line| !line.is_empty());
    lines
}

impl<'a> BlockRenderer<'a> {
    fn render_quote(&mut self, blocks: Vec<ParsedBlock>) -> AnyElement {
        let border: Rgba = self.theme.markdown.blockquote_border;
        let mut body = div()
            .w_full()
            .pl_3()
            .border_l_2()
            .border_color(border)
            .flex()
            .flex_col()
            .gap_1();
        for block in blocks {
            match block {
                ParsedBlock::Paragraph(runs) => {
                    let muted: Rgba = self.theme.markdown.muted;
                    let style = SelectableStyle {
                        size: self.theme.typography.text_sm,
                        line_height: crate::theme::leading_relaxed(self.theme),
                        color: muted,
                        weight: FontWeight::NORMAL,
                        italic: true,
                        family: self.ui_family(),
                        align: TextAlign::Left,
                    };
                    let el = self.inline_text(runs, style, true);
                    body = body.child(div().cursor(CursorStyle::IBeam).child(el));
                }
                other => {
                    body = body.child(self.render_parsed(other));
                }
            }
        }
        body.into_any_element()
    }
}

impl<'a> BlockRenderer<'a> {
    fn render_code(&mut self, lang: &str, source: &str) -> AnyElement {
        use crate::ui::multiline_text::split_display_lines;
        let code_bg: Rgba = self.theme.markdown.code_bg;
        let border: Rgba = self.theme.markdown.border;
        let muted: Rgba = self.theme.markdown.muted;
        let text_color: Rgba = self.theme.markdown.text;
        let mono = crate::theme::font_mono(self.theme);
        let scroll_id = self.next_scroll_id("code");
        let mut outer = div()
            .id(ElementId::Name(scroll_id.into()))
            .w_full()
            .rounded_md()
            .bg(code_bg)
            .border_1()
            .border_color(border)
            .overflow_x_scroll()
            .flex()
            .flex_col();

        if !lang.is_empty() {
            outer = outer.child(
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .text_color(muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child(lang.to_string()),
            );
        }

        // Selectable lines (no wrap: the scroll container handles overflow).
        // `items_start` keeps short code visually identical to before.
        let mut content = div().flex().flex_col().items_start().gap_1().p_2();
        for line in split_display_lines(source.trim_end()) {
            let style = SelectableStyle {
                size: self.theme.typography.text_xs,
                line_height: crate::theme::leading_relaxed(self.theme),
                color: text_color,
                weight: FontWeight::NORMAL,
                italic: false,
                family: mono.clone(),
                align: TextAlign::Left,
            };
            let key = self.next_text_key();
            content = content.child(div().cursor(CursorStyle::IBeam).child(selectable_text(
                &self.view,
                key,
                &[InlineRun {
                    styles: vec![InlineStyle::Normal],
                    text: line,
                    url: None,
                }],
                self.theme,
                &style,
                false,
            )));
        }
        outer.child(content).into_any_element()
    }
}

impl<'a> BlockRenderer<'a> {
    fn render_list(
        &mut self,
        items: Vec<Vec<ParsedBlock>>,
        ordered: bool,
        start: u64,
    ) -> AnyElement {
        let muted: Rgba = self.theme.markdown.muted;
        let mut list = div().w_full().flex().flex_col().gap_1().pl_2();
        for (index, blocks) in items.into_iter().enumerate() {
            let marker = if ordered {
                format!("{}. ", start + index as u64)
            } else {
                "• ".to_string()
            };
            let mut content = div().flex_1().min_w(px(0.)).flex().flex_col().gap_1();
            for block in blocks {
                content = content.child(self.render_parsed(block));
            }
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .text_color(muted)
                            .child(marker),
                    )
                    .child(content),
            );
        }
        list.into_any_element()
    }
}

impl<'a> BlockRenderer<'a> {
    fn render_table(
        &mut self,
        header: &[Vec<InlineRun>],
        rows: &[Vec<Vec<InlineRun>>],
        alignments: &[Alignment],
    ) -> AnyElement {
        let border: Rgba = self.theme.markdown.border;
        let scroll_id = self.next_scroll_id("table");
        let mut inner = div().w_full().rounded_md().border_1().border_color(border);
        // NOTE: no `overflow_hidden` here — it would clip unbreakable content
        // inside and starve the outer horizontal scroll.

        if !header.is_empty() {
            inner = inner.child(self.render_table_row(header, alignments, true, true));
        }
        let last = rows.len().saturating_sub(1);
        for (index, row) in rows.iter().enumerate() {
            let is_last = index == last;
            inner = inner.child(self.render_table_row(row, alignments, false, is_last));
        }
        // Issue #11: horizontal scroll engages only when unbreakable content
        // overflows; fitting tables render exactly as before.
        div()
            .id(ElementId::Name(scroll_id.into()))
            .w_full()
            .overflow_x_scroll()
            .child(inner)
            .into_any_element()
    }
}

impl<'a> BlockRenderer<'a> {
    fn render_table_row(
        &mut self,
        cells: &[Vec<InlineRun>],
        alignments: &[Alignment],
        header: bool,
        is_last: bool,
    ) -> Div {
        let md_text: Rgba = self.theme.markdown.text;
        let md_muted: Rgba = self.theme.markdown.muted;
        let md_code_bg: Rgba = self.theme.markdown.code_bg;
        let md_border: Rgba = self.theme.markdown.border;
        let mut row = div().w_full().flex().flex_row().border_color(md_border);
        if !is_last {
            row = row.border_b_1();
        }
        for (index, cell) in cells.iter().enumerate() {
            let align = alignments.get(index).copied().unwrap_or(Alignment::None);
            let g_align = match align {
                Alignment::Center => TextAlign::Center,
                Alignment::Right => TextAlign::Right,
                _ => TextAlign::Left,
            };
            let mut cell_el = div()
                .flex_1()
                // 窄列保底 80px：多列表格触发外层横向滚动，而不是把每列
                // 挤到几 px 后触发强制断行（曾表现为竖排单字）。
                .min_w(px(80.))
                .px_2()
                .py_1()
                .cursor(CursorStyle::IBeam);
            if header {
                cell_el = cell_el.bg(md_code_bg);
            }
            // Issue #6: cells render inline runs (bold/code/links), not raw
            // text — now drag-selectable with alignment preserved.
            let mut style = SelectableStyle {
                size: self.theme.typography.text_xs,
                line_height: crate::theme::leading_relaxed(self.theme),
                color: if header { md_text } else { md_muted },
                weight: FontWeight::NORMAL,
                italic: false,
                family: self.ui_family(),
                align: g_align,
            };
            if header {
                style.weight = FontWeight::SEMIBOLD;
            }
            let key = self.next_text_key();
            let el = selectable_text(&self.view, key, cell, self.theme, &style, true);
            row = row.child(cell_el.child(el));
        }
        row
    }
}

#[cfg(test)]
mod tests {
    // NOTE: never `use super::*` here — the parent's `use gpui::*` would pull
    // gpui's `test` module into scope and break `#[test]` resolution.
    use super::{
        is_line_break_html, push_run_into, split_runs_on_newlines, strip_html_tags, trim_cell_runs,
        InlineRun, InlineStyle, ParseContext, ParsedBlock,
    };
    use crate::selectable_text::is_browsable_url;
    use crate::theme::themes::dark::dark_tokens;
    use crate::theme::ThemeTokens;
    use pulldown_cmark::{Alignment, Event, Tag, TagEnd};

    fn test_theme() -> ThemeTokens {
        dark_tokens()
    }

    /// Drive the REAL render entry point (including the hard-wrap reflow
    /// pre-pass), returning the data tree (no theme/view needed).
    ///
    /// NOTE: this must call `parse_blocks`, not a re-implemented parser —
    /// a local duplicate silently skipped `reflow_hard_wrapped`, so every
    /// reflow test would have passed while the UI still broke words.
    fn parse(source: &str) -> Vec<ParsedBlock> {
        super::parse_blocks(source)
    }

    fn is_heading(block: &ParsedBlock) -> bool {
        matches!(block, ParsedBlock::Heading { .. })
    }

    fn paragraph_text(block: &ParsedBlock) -> String {
        match block {
            ParsedBlock::Paragraph(runs) => runs.iter().map(|r| r.text.as_str()).collect(),
            _ => panic!("expected paragraph, got {block:?}"),
        }
    }

    #[test]
    fn heading_with_mixed_styles_stays_one_block() {
        // Issue #1: `### 1. 教学主循环（`teaching-loop.ts`）` must not split.
        let blocks = parse("### 1. 教学主循环（`teaching-loop.ts`）\n");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ParsedBlock::Heading { level, runs } => {
                assert_eq!(*level, 3);
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                assert!(text.contains("teaching-loop.ts"));
                assert!(runs.iter().any(|r| r.styles.contains(&InlineStyle::Code)));
            }
            other => panic!("expected heading, got {other:?}"),
        }
        assert!(is_heading(&blocks[0]));
    }

    #[test]
    fn nested_list_stays_single_block() {
        // Issue #3: inner list must not wipe outer items or escape.
        let blocks = parse("- a\n  - b\n  - c\n- d\n");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ParsedBlock::List { items, .. } => {
                assert_eq!(items.len(), 2);
                assert_eq!(paragraph_text(&items[0][0]), "a");
                assert_eq!(paragraph_text(&items[1][0]), "d");
                // Nested list is the second block of the first item.
                assert_eq!(items[0].len(), 2);
                match &items[0][1] {
                    ParsedBlock::List { items: inner, .. } => {
                        assert_eq!(inner.len(), 2);
                        assert_eq!(paragraph_text(&inner[0][0]), "b");
                        assert_eq!(paragraph_text(&inner[1][0]), "c");
                    }
                    other => panic!("expected nested list, got {other:?}"),
                }
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn multi_paragraph_item_keeps_both_paragraphs() {
        // Issue #4: both paragraphs survive with content (old code dropped
        // the first via overwrite).
        let blocks = parse("- first\n\n  continued\n");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ParsedBlock::List { items, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].len(), 2);
                assert_eq!(paragraph_text(&items[0][0]), "first");
                assert_eq!(paragraph_text(&items[0][1]), "continued");
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn code_block_inside_list_item_does_not_escape() {
        // Issue #5: code keeps language + source inside the item.
        let blocks = parse("- item\n\n  ```rust\n  let x = 1;\n  ```\n");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ParsedBlock::List { items, .. } => {
                assert_eq!(items[0].len(), 2);
                match &items[0][1] {
                    ParsedBlock::Code { lang, source } => {
                        assert_eq!(lang, "rust");
                        assert!(source.contains("let x = 1;"));
                    }
                    other => panic!("expected code, got {other:?}"),
                }
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn table_inside_list_item_does_not_escape() {
        // Issue #5.
        let blocks = parse("- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n");
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn list_inside_blockquote_stays_single_block() {
        let blocks = parse("> quote\n>\n> - a\n> - b\n");
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn image_becomes_clickable_placeholder_run() {
        // Issue #7: alt captured, url attached, nothing leaks as bare text.
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Paragraph));
        ctx.handle(Event::Start(Tag::Image {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "https://x.test/i.png".into(),
            title: "".into(),
            id: "".into(),
        }));
        ctx.handle(Event::Text("diagram".into()));
        ctx.handle(Event::End(TagEnd::Image));
        assert!(ctx.image_stack.is_empty());
        assert_eq!(ctx.inlines.len(), 1);
        let run = &ctx.inlines[0];
        assert_eq!(run.styles, vec![InlineStyle::Image]);
        assert_eq!(run.url.as_deref(), Some("https://x.test/i.png"));
        assert!(run.text.contains("diagram"));
        ctx.handle(Event::End(TagEnd::Paragraph));
    }

    #[test]
    fn br_html_becomes_hard_break() {
        // Issue #8.
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Paragraph));
        ctx.handle(Event::Text("a".into()));
        ctx.handle(Event::InlineHtml("<br/>".into()));
        ctx.handle(Event::Text("b".into()));
        assert_eq!(ctx.inlines.len(), 1);
        assert_eq!(ctx.inlines[0].text, "a\nb");
    }

    #[test]
    fn link_runs_carry_url_and_plain_text_does_not() {
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Paragraph));
        ctx.handle(Event::Text("see ".into()));
        ctx.handle(Event::Start(Tag::Link {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "https://x.test".into(),
            title: "".into(),
            id: "".into(),
        }));
        ctx.handle(Event::Text("docs".into()));
        ctx.handle(Event::End(TagEnd::Link));
        assert_eq!(ctx.inlines.len(), 2);
        assert_eq!(ctx.inlines[0].url, None);
        assert_eq!(ctx.inlines[1].url.as_deref(), Some("https://x.test"));
        assert!(ctx.inlines[1].styles.contains(&InlineStyle::Link));
    }

    #[test]
    fn nested_emphasis_keeps_whole_stack() {
        // Issue #13: `***x***` keeps both Emphasis and Strong.
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Paragraph));
        ctx.handle(Event::Start(Tag::Emphasis));
        ctx.handle(Event::Start(Tag::Strong));
        ctx.handle(Event::Text("x".into()));
        assert_eq!(
            ctx.inlines[0].styles,
            vec![InlineStyle::Emphasis, InlineStyle::Strong]
        );
    }

    #[test]
    fn table_cells_keep_inline_styles() {
        // Issue #6: `**b**` in a cell parses to a Strong run, not raw text.
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Table(vec![Alignment::None])));
        ctx.handle(Event::Start(Tag::TableHead));
        ctx.handle(Event::Start(Tag::TableRow));
        ctx.handle(Event::Start(Tag::TableCell));
        ctx.handle(Event::Text("h".into()));
        ctx.handle(Event::End(TagEnd::TableCell));
        ctx.handle(Event::End(TagEnd::TableRow));
        ctx.handle(Event::End(TagEnd::TableHead));
        ctx.handle(Event::Start(Tag::TableRow));
        ctx.handle(Event::Start(Tag::TableCell));
        ctx.handle(Event::Start(Tag::Strong));
        ctx.handle(Event::Text("b".into()));
        ctx.handle(Event::End(TagEnd::Strong));
        ctx.handle(Event::End(TagEnd::TableCell));
        ctx.handle(Event::End(TagEnd::TableRow));
        assert_eq!(ctx.table_rows.len(), 1);
        let cell = &ctx.table_rows[0][0];
        assert!(cell.iter().any(|r| r.styles.contains(&InlineStyle::Strong)));
        assert!(cell.iter().all(|r| !r.text.contains("**")));
        ctx.handle(Event::End(TagEnd::Table));
        assert!(ctx.table_rows.is_empty());
    }

    #[test]
    fn line_break_html_detection() {
        assert!(is_line_break_html("<br>"));
        assert!(is_line_break_html("<BR/>"));
        assert!(is_line_break_html("  <br />"));
        assert!(!is_line_break_html("<b>"));
        assert!(!is_line_break_html("<blockquote>"));
        assert!(!is_line_break_html("br"));
    }

    #[test]
    fn strip_html_tags_drops_tags_keeps_text() {
        assert_eq!(strip_html_tags("<div>hi</div>").trim(), "hi");
        assert_eq!(strip_html_tags("<br/>"), "");
    }

    #[test]
    fn browsable_url_gate() {
        assert!(is_browsable_url("https://x.test/a"));
        assert!(is_browsable_url("http://x.test"));
        assert!(!is_browsable_url("javascript:alert(1)"));
        assert!(!is_browsable_url("ftp://x.test"));
        assert!(!is_browsable_url(""));
    }

    #[test]
    fn push_run_merges_identical_neighbors() {
        let mut runs = Vec::new();
        push_run_into(&mut runs, vec![InlineStyle::Normal], None, "a");
        push_run_into(&mut runs, vec![InlineStyle::Normal], None, "b");
        push_run_into(&mut runs, vec![InlineStyle::Strong], None, "c");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "ab");
    }

    #[test]
    fn split_runs_preserves_styles_and_urls() {
        let runs = vec![InlineRun {
            styles: vec![InlineStyle::Strong, InlineStyle::Link],
            text: "a\nb".into(),
            url: Some("https://x.test".into()),
        }];
        let lines = split_runs_on_newlines(runs);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert_eq!(line[0].styles, vec![InlineStyle::Strong, InlineStyle::Link]);
            assert_eq!(line[0].url.as_deref(), Some("https://x.test"));
        }
    }

    #[test]
    fn trim_cell_runs_trims_edges_only() {
        let mut cell = vec![
            InlineRun {
                styles: vec![InlineStyle::Normal],
                text: "  ".into(),
                url: None,
            },
            InlineRun {
                styles: vec![InlineStyle::Normal],
                text: "x".into(),
                url: None,
            },
            InlineRun {
                styles: vec![InlineStyle::Normal],
                text: "  ".into(),
                url: None,
            },
        ];
        trim_cell_runs(&mut cell);
        assert_eq!(cell.len(), 1);
        assert_eq!(cell[0].text, "x");
    }

    #[test]
    fn composite_document_yields_exact_top_blocks() {
        // Heading + paragraph + nested list + code + table + quote = 6.
        // Any escape/duplication across structural paths changes the count.
        let blocks = parse(
            "# Title\n\nIntro para.\n\n- one\n- two\n  - nested\n\n```rust\nlet x = 1;\n```\n\n| h1 | h2 |\n|---|---|\n| a | b |\n\n> quoted\n",
        );
        assert_eq!(blocks.len(), 6);
        assert!(is_heading(&blocks[0]));
    }

    #[test]
    fn image_inside_table_cell_stays_in_cell() {
        let mut ctx = ParseContext::new();
        ctx.handle(Event::Start(Tag::Table(vec![Alignment::None])));
        ctx.handle(Event::Start(Tag::TableRow));
        ctx.handle(Event::Start(Tag::TableCell));
        ctx.handle(Event::Start(Tag::Image {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "https://x.test/i.png".into(),
            title: "".into(),
            id: "".into(),
        }));
        ctx.handle(Event::Text("pic".into()));
        ctx.handle(Event::End(TagEnd::Image));
        ctx.handle(Event::End(TagEnd::TableCell));
        assert!(ctx.inlines.is_empty());
        let cell = &ctx.table_row[0];
        assert!(cell.iter().any(|r| r.styles == vec![InlineStyle::Image]));
    }

    #[test]
    fn build_runs_maps_styles_to_fonts_and_links() {
        use crate::selectable_text::{build_runs, SelectableStyle};
        use gpui::{FontWeight, SharedString, TextAlign};
        let theme = test_theme();
        let style = SelectableStyle {
            size: theme.typography.text_xs,
            line_height: crate::theme::leading_relaxed(&theme),
            color: theme.markdown.text,
            weight: FontWeight::NORMAL,
            italic: false,
            family: SharedString::from("ui"),
            align: TextAlign::Left,
        };
        let inline = vec![
            InlineRun {
                styles: vec![InlineStyle::Strong, InlineStyle::Link],
                text: "docs".into(),
                url: Some("https://x.test".into()),
            },
            InlineRun {
                styles: vec![InlineStyle::Code],
                text: "x".into(),
                url: None,
            },
        ];
        let (text, runs) = build_runs(&inline, &theme, &style);
        assert_eq!(text, "docsx");
        assert_eq!(runs[0].font.weight, FontWeight::SEMIBOLD);
        assert!(runs[0].underline);
        assert_eq!(runs[0].link.as_deref(), Some("https://x.test"));
        assert!(runs[1].background.is_some());
        assert!(!runs[1].underline);
    }

    #[test]
    fn soft_break_keeps_line_break_for_chat_output() {
        // 中文 LLM 常用单 `\n` 分段；按 Markdown 空格合并会挤成一大段。
        let blocks = parse("第一行\n第二行\n");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ParsedBlock::Paragraph(runs) => {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                assert!(text.contains('\n'), "soft break lost: {text:?}");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn reflow_joins_model_hard_wrap_mid_word() {
        // 截图问题：模型按列硬换行，`编译不` / `过` / `。` 被拆成孤行。
        let source = concat!(
            "1. **无法验证编译**: tsc 和 jest 在本机全部超时（各自 300s+ 被阻塞）。",
            "teaching-context.ts 的改动引入了 MAX_HISTORY_MESSAGES / KEEP_RECENT_MESSAGES ",
            "常量和 addToolRound，但 Message\n",
            "类型还没有 toolCallId 字段（packages/agent-core/src/types.ts 的 Message 接口）",
            "—— 当前代码处于类型不一致状态，很可能编译不\n",
            "过\n",
            "。\n"
        );
        let out = super::reflow_hard_wrapped(source);
        // 全部三行必须合回一段，且 CJK 之间不插空格。
        assert!(
            out.contains("很可能编译不过。"),
            "mid-word wrap not rejoined: {out:?}"
        );
        assert!(!out.contains("编译不\n过"), "orphan line survived: {out:?}");
        assert!(!out.contains("但 Message\n类型"), "wrap survived: {out:?}");
    }

    #[test]
    fn reflow_keeps_short_authored_line_breaks() {
        // 短行是作者真实换行（`1.` / `**粗体**` / 短句），不得合并。
        let source = "我必须向你们坦白一个关键情况：\n本次修复未能完成。\n请重试。\n";
        assert_eq!(super::reflow_hard_wrapped(source), source);
    }

    #[test]
    fn reflow_keeps_fenced_code_byte_exact() {
        let source = "说明文字很长很长很长很长很长很长很长很长很长很长很长很长\n\
                     ```rust\nlet x = 1;\nlet y = 2;\n```\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("let x = 1;\nlet y = 2;"), "{out:?}");
    }

    #[test]
    fn reflow_keeps_table_rows() {
        let source = "这是一段很长的说明文字用于测试表格不被合并的逻辑路径分支\n\
                     | a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("| a | b |\n|---|---|\n| 1 | 2 |"), "{out:?}");
    }

    #[test]
    fn reflow_inserts_space_between_latin_words() {
        let source = "the tool registry resolves every name by a hardcoded mapping table value\n\
                     that never learns the new ones at runtime\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("value that never"), "{out:?}");
    }

    #[test]
    fn reflow_never_splits_dangling_inline_markers() {
        // `**粗体` / `` `code `` 的闭合标记在下一行：合并会让闭合标记落到行首，
        // 破坏强调与行内代码。intraword `_`（MAX_HISTORY_MESSAGES）不算标记。
        let source =
            "这是一段足够长的说明文字用来触发硬换行合并逻辑的阈值条件判断 **粗\n体** 收尾\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(
            out.contains("**粗\n体**"),
            "dangling marker joined: {out:?}"
        );

        let code = "这是一段足够长的说明文字用来触发硬换行合并逻辑的阈值条件判断 MAX_HISTORY 变量 `na\nme` 收尾\n";
        let out = super::reflow_hard_wrapped(code);
        assert!(out.contains("`na\nme`"), "dangling code joined: {out:?}");
    }

    #[test]
    fn reflow_treats_balanced_markers_as_joinable() {
        // 闭合完整的 `**bold**` 可以在行首，不是悬挂标记 → 允许合并。
        let source = "这是一段足够长的说明文字用来触发硬换行合并逻辑的阈值条件判断\n**bold continuation here**\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("判断 **bold continuation here**"), "{out:?}");
    }

    /// 截图原文（逐字），走真实渲染入口 `parse_blocks`。
    ///
    /// 这是唯一能捕获「reflow 存在但没接进渲染路径」的测试 —— 上一轮的
    /// reflow 测试全部直接调用函数，因此集成层的回归无人发现。
    #[test]
    fn screenshot_document_has_no_orphan_wrapped_lines() {
        let source = concat!(
            "## 为什么我停下来\n",
            "\n",
            "按照系统指令：「If you make a mistake, acknowledge it and move on. ",
            "Only produce valid output.」[Never fabricate results.]\n",
            "\n",
            "我面临三个不能忽视的风险:\n",
            "\n",
            "1. **无法验证编译**: tsc 和 jest 在本机全部超时（各自 300s+ 被阻塞）。",
            "teaching-context.ts 的改动引入了 MAX_HISTORY_MESSAGES / ",
            "KEEP_RECENT_MESSAGES 常量和 addToolRound，但 Message\n",
            "   类型还没有 toolCallId 字段（packages/agent-core/src/types.ts 的 ",
            "Message 接口）—— 当前代码处于类型不一致状态，很可能编译不\n",
            "过\n",
            "。\n",
            "\n",
            "2. **不能假装完成**: 如果我继续将「剩余文件」但实际写入失败。\n",
        );
        let blocks = parse(source);
        // 收集所有段落文本，断言不存在孤立的「过」/「。」行。
        let mut texts: Vec<String> = Vec::new();
        for block in &blocks {
            match block {
                ParsedBlock::Paragraph(runs) | ParsedBlock::Heading { runs, .. } => {
                    texts.push(runs.iter().map(|r| r.text.as_str()).collect());
                }
                ParsedBlock::List { items, .. } => {
                    for item in items {
                        for sub in item {
                            if let ParsedBlock::Paragraph(runs) = sub {
                                texts.push(runs.iter().map(|r| r.text.as_str()).collect());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let all = texts.join("\n");
        assert!(
            all.contains("很可能编译不过。"),
            "mid-word wrap survived into blocks: {all:?}"
        );
        // 不得出现「单独成行的过 / 。」——即行首即为该字且行长极短。
        for text in &texts {
            for line in text.split('\n') {
                let trimmed = line.trim();
                assert!(
                    !(trimmed == "过" || trimmed == "。"),
                    "orphan line survived: {text:?}"
                );
            }
        }
        // 列表结构与第 2 项必须完好。
        assert!(all.contains("不能假装完成"), "{all:?}");
        // heading + 2 paragraphs + 1 list。
        assert_eq!(blocks.len(), 4, "heading + paras + list: {all:?}");
    }

    #[test]
    fn reflow_preserves_explicit_hard_break() {
        // 行尾两空格 = Markdown 显式硬换行，必须保留。
        let source = "这是一段足够长的说明文字用来触发硬换行合并逻辑的阈值条件判断  \n下一行\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("  \n下一行"), "{out:?}");
    }

    #[test]
    fn reflow_keeps_list_structure() {
        let source = "这是一段足够长的说明文字用来触发硬换行合并逻辑的阈值条件判断\n\
                     - 第一项\n- 第二项\n";
        let out = super::reflow_hard_wrapped(source);
        assert!(out.contains("\n- 第一项\n- 第二项"), "{out:?}");
    }

    #[test]
    fn soft_break_lines_split_for_render() {
        let runs = vec![InlineRun {
            styles: vec![InlineStyle::Normal],
            text: "a\nb".into(),
            url: None,
        }];
        let lines = split_runs_on_newlines(runs);
        assert_eq!(lines.len(), 2);
    }
}
