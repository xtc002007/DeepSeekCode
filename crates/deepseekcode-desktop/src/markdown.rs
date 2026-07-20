use crate::theme::ThemeTokens;
use crate::ui::multiline_text::render_multiline;
use gpui::*;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

pub fn render_markdown(source: &str, theme: &ThemeTokens) -> impl IntoElement {
    let blocks = parse_blocks(source, theme);
    let leading = crate::theme::leading_relaxed(theme);
    let mut container = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .line_height(leading);
    for block in blocks {
        container = container.child(block);
    }
    container
}

fn parse_blocks(source: &str, theme: &ThemeTokens) -> Vec<AnyElement> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(source, options);
    let mut ctx = ParseContext::new(theme);

    for event in parser {
        ctx.handle(event);
    }
    ctx.finish()
}

struct ParseContext<'a> {
    theme: &'a ThemeTokens,
    blocks: Vec<AnyElement>,
    inlines: Vec<InlineRun>,
    inline_stack: Vec<InlineStyle>,
    heading_level: Option<u8>,
    in_code_block: bool,
    code_lang: String,
    code_lines: String,
    list_items: Vec<Vec<InlineRun>>,
    current_item: Vec<InlineRun>,
    list_ordered: bool,
    in_list: bool,
    in_blockquote: bool,
    blockquote_lines: Vec<Vec<InlineRun>>,
    table_header: Vec<String>,
    table_rows: Vec<Vec<String>>,
    table_row: Vec<String>,
    table_cell: String,
    in_table: bool,
    in_table_head: bool,
    table_alignments: Vec<Alignment>,
}

impl<'a> ParseContext<'a> {
    fn new(theme: &'a ThemeTokens) -> Self {
        Self {
            theme,
            blocks: Vec::new(),
            inlines: Vec::new(),
            inline_stack: Vec::new(),
            heading_level: None,
            in_code_block: false,
            code_lang: String::new(),
            code_lines: String::new(),
            list_items: Vec::new(),
            current_item: Vec::new(),
            list_ordered: false,
            in_list: false,
            in_blockquote: false,
            blockquote_lines: Vec::new(),
            table_header: Vec::new(),
            table_rows: Vec::new(),
            table_row: Vec::new(),
            table_cell: String::new(),
            in_table: false,
            in_table_head: false,
            table_alignments: Vec::new(),
        }
    }

    fn flush_inlines(&mut self) {
        if self.inlines.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.inlines);
        if let Some(level) = self.heading_level.take() {
            self.blocks.push(render_heading(level, runs, self.theme));
        } else if self.in_list {
            self.current_item = runs;
        } else if self.in_blockquote {
            self.blockquote_lines.push(runs);
        } else {
            self.blocks.push(render_paragraph(runs, self.theme));
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_lines.push_str(text);
            return;
        }
        if self.in_table {
            self.table_cell.push_str(text);
            return;
        }
        let style = self
            .inline_stack
            .last()
            .copied()
            .unwrap_or(InlineStyle::Normal);
        if let Some(last) = self.inlines.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }
        self.inlines.push(InlineRun {
            style,
            text: text.to_string(),
        });
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
                    self.in_list = true;
                    self.list_ordered = start.is_some();
                    self.list_items.clear();
                    self.current_item.clear();
                }
                Tag::Item => {
                    self.flush_inlines();
                    self.current_item.clear();
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
                    self.in_blockquote = true;
                    self.blockquote_lines.clear();
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
                Tag::Link { .. } => self.inline_stack.push(InlineStyle::Link),
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
                    self.list_items.push(std::mem::take(&mut self.current_item));
                }
                TagEnd::List(_) => {
                    self.in_list = false;
                    let items = std::mem::take(&mut self.list_items);
                    self.blocks
                        .push(render_list(items, self.list_ordered, self.theme));
                }
                TagEnd::CodeBlock => {
                    self.in_code_block = false;
                    self.blocks.push(render_code_block(
                        &self.code_lines,
                        &self.code_lang,
                        self.theme,
                    ));
                    self.code_lines.clear();
                    self.code_lang.clear();
                }
                TagEnd::BlockQuote(_) => {
                    self.flush_inlines();
                    self.in_blockquote = false;
                    let lines = std::mem::take(&mut self.blockquote_lines);
                    self.blocks.push(render_blockquote(lines, self.theme));
                }
                TagEnd::Table => {
                    self.in_table = false;
                    self.blocks.push(render_table(
                        &self.table_header,
                        &self.table_rows,
                        &self.table_alignments,
                        self.theme,
                    ));
                    self.table_header.clear();
                    self.table_rows.clear();
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
                    self.table_row
                        .push(self.table_cell.trim().to_string());
                    self.table_cell.clear();
                }
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link => {
                    self.inline_stack.pop();
                }
                _ => {}
            },
            Event::Text(text) => self.push_text(&text),
            Event::Code(text) => {
                self.inline_stack.push(InlineStyle::Code);
                self.push_text(&text);
                self.inline_stack.pop();
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_inlines();
                self.blocks.push(
                    div()
                        .w_full()
                        .h(px(1.))
                        .bg(self.theme.markdown.border)
                        .into_any_element(),
                );
            }
            Event::TaskListMarker(checked) => {
                self.push_text(if checked { "☑ " } else { "☐ " });
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<AnyElement> {
        self.flush_inlines();
        self.blocks
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineStyle {
    Normal,
    Strong,
    Emphasis,
    Code,
    Link,
    Strike,
}

struct InlineRun {
    style: InlineStyle,
    text: String,
}

fn render_heading(level: u8, runs: Vec<InlineRun>, theme: &ThemeTokens) -> AnyElement {
    let md = &theme.markdown;
    let mut el = div()
        .w_full()
        .font_weight(FontWeight::BOLD)
        .text_color(md.text);
    el = match level {
        1 => el.text_lg(),
        2 => el.text_base(),
        _ => el.text_sm(),
    };
    render_runs_into(el, runs, theme).into_any_element()
}

fn render_paragraph(runs: Vec<InlineRun>, theme: &ThemeTokens) -> AnyElement {
    let md = &theme.markdown;
    let mut column = div()
        .w_full()
        .flex()
        .flex_col()
        .gap_0p5()
        .text_xs()
        .line_height(crate::theme::leading_relaxed(theme))
        .text_color(md.text);
    for line_runs in split_runs_on_newlines(runs) {
        column = column.child(render_inline_line(line_runs, theme));
    }
    column.into_any_element()
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
            lines
                .last_mut()
                .expect("line vector non-empty")
                .push(InlineRun {
                    text: segment.to_string(),
                    style: run.style,
                });
        }
    }
    if lines.len() == 1 && lines[0].is_empty() {
        return vec![];
    }
    lines.retain(|line| !line.is_empty());
    lines
}

fn render_inline_line(runs: Vec<InlineRun>, theme: &ThemeTokens) -> Div {
    render_runs_into(
        div()
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_baseline(),
        runs,
        theme,
    )
}

fn render_blockquote(lines: Vec<Vec<InlineRun>>, theme: &ThemeTokens) -> AnyElement {
    let md = &theme.markdown;
    let mut body = div()
        .w_full()
        .pl_3()
        .border_l_2()
        .border_color(md.blockquote_border)
        .flex()
        .flex_col()
        .gap_1();
    for line in lines {
        body = body.child(
            render_inline_line(line, theme)
                .text_sm()
                .text_color(md.muted)
                .italic(),
        );
    }
    body.into_any_element()
}

fn render_runs_into(mut container: Div, runs: Vec<InlineRun>, theme: &ThemeTokens) -> Div {
    let md = &theme.markdown;
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let mut span = div().flex_shrink_0().child(run.text.clone());
        span = match run.style {
            InlineStyle::Normal => span,
            InlineStyle::Strong => span.font_weight(FontWeight::SEMIBOLD),
            InlineStyle::Emphasis => span.italic(),
            InlineStyle::Code => span
                .px_1()
                .rounded_sm()
                .bg(md.code_bg)
                .font_family(crate::theme::font_mono(theme))
                .text_xs()
                .text_color(md.text),
            InlineStyle::Link => span.text_color(md.link).underline(),
            InlineStyle::Strike => span.line_through(),
        };
        container = container.child(span);
    }
    container
}

fn render_code_block(source: &str, lang: &str, theme: &ThemeTokens) -> AnyElement {
    let md = &theme.markdown;
    let mut block = div()
        .w_full()
        .p_2()
        .rounded_md()
        .bg(md.code_bg)
        .border_1()
        .border_color(md.border)
        .flex()
        .flex_col()
        .gap_1();

    if !lang.is_empty() {
        block = block.child(
            div()
                .text_xs()
                .text_color(md.muted)
                .font_weight(FontWeight::MEDIUM)
                .child(lang.to_string()),
        );
    }

    let mono = crate::theme::font_mono(theme);
    block
        .child(render_multiline(
            source.trim_end(),
            move |el| el.text_xs().font_family(mono.clone()).text_color(md.text),
        ))
        .into_any_element()
}

fn render_list(items: Vec<Vec<InlineRun>>, ordered: bool, theme: &ThemeTokens) -> AnyElement {
    let md = &theme.markdown;
    let mut list = div().w_full().flex().flex_col().gap_1().pl_2();
    for (index, item) in items.into_iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", index + 1)
        } else {
            "• ".to_string()
        };
        list = list.child(
            div()
                .flex()
                .gap_1()
                .child(div().flex_shrink_0().text_sm().text_color(md.muted).child(marker))
                .child(render_paragraph(item, theme)),
        );
    }
    list.into_any_element()
}

fn render_table(
    header: &[String],
    rows: &[Vec<String>],
    alignments: &[Alignment],
    theme: &ThemeTokens,
) -> AnyElement {
    let md = &theme.markdown;
    let mut table = div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(md.border)
        .overflow_hidden();

    if !header.is_empty() {
        table = table.child(render_table_row(header, alignments, true, theme));
    }
    for row in rows {
        table = table.child(render_table_row(row, alignments, false, theme));
    }
    table.into_any_element()
}

fn render_table_row(
    cells: &[String],
    alignments: &[Alignment],
    header: bool,
    theme: &ThemeTokens,
) -> impl IntoElement {
    let md = &theme.markdown;
    let mut row = div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(md.border);
    for (index, cell) in cells.iter().enumerate() {
        let align = alignments.get(index).copied().unwrap_or(Alignment::None);
        let mut cell_el = div()
            .flex_1()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(if header { md.text } else { md.muted });
        if header {
            cell_el = cell_el.font_weight(FontWeight::SEMIBOLD).bg(md.code_bg);
        }
        cell_el = match align {
            Alignment::Center => cell_el.text_center(),
            Alignment::Right => cell_el.text_right(),
            _ => cell_el,
        };
        row = row.child(cell_el.child(cell.clone()));
    }
    row
}
