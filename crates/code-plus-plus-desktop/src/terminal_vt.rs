//! PTY output grid using `alacritty_terminal` (plan §6.4).

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalColor(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct ColoredSpan {
    pub text: String,
    pub fg: TerminalColor,
    /// Background override; `None` means transparent (use `terminal_bg`).
    pub bg: Option<TerminalColor>,
    pub bold: bool,
    pub dim: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ColoredLine {
    pub spans: Vec<ColoredSpan>,
}

pub fn terminal_default_fg() -> TerminalColor {
    TerminalColor(0xCCCCCC)
}

/// Convert a theme color into terminal palette space (drops alpha).
///
/// Used for synthetic spans the PTY never emitted — e.g. the block cursor —
/// so they follow light/dark themes instead of hardcoding one palette.
pub fn terminal_color_from_rgba(c: gpui::Rgba) -> TerminalColor {
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    TerminalColor((channel(c.r) << 16) | (channel(c.g) << 8) | channel(c.b))
}

pub fn terminal_default_bg() -> TerminalColor {
    TerminalColor(0x000000)
}

fn rgb_hex(r: u8, g: u8, b: u8) -> TerminalColor {
    TerminalColor(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
}

fn color_to_rgb(color: Color) -> TerminalColor {
    match color {
        Color::Named(named) => match named {
            NamedColor::Black => rgb_hex(0x0C, 0x0C, 0x0C),
            NamedColor::Red => rgb_hex(0xC5, 0x0F, 0x1F),
            NamedColor::Green => rgb_hex(0x13, 0xA1, 0x0E),
            NamedColor::Yellow => rgb_hex(0xC1, 0x9C, 0x00),
            NamedColor::Blue => rgb_hex(0x00, 0x37, 0xDA),
            NamedColor::Magenta => rgb_hex(0x88, 0x17, 0x98),
            NamedColor::Cyan => rgb_hex(0x3A, 0x96, 0xDD),
            NamedColor::White => rgb_hex(0xCC, 0xCC, 0xCC),
            NamedColor::BrightBlack => rgb_hex(0x76, 0x76, 0x76),
            NamedColor::BrightRed => rgb_hex(0xE7, 0x48, 0x56),
            NamedColor::BrightGreen => rgb_hex(0x16, 0xC6, 0x0C),
            NamedColor::BrightYellow => rgb_hex(0xF9, 0xF1, 0xA5),
            NamedColor::BrightBlue => rgb_hex(0x3B, 0x78, 0xFF),
            NamedColor::BrightMagenta => rgb_hex(0xB4, 0x00, 0x9E),
            NamedColor::BrightCyan => rgb_hex(0x61, 0xD6, 0xD6),
            NamedColor::BrightWhite => rgb_hex(0xF2, 0xF2, 0xF2),
            NamedColor::Foreground => terminal_default_fg(),
            NamedColor::Background => terminal_default_bg(),
            NamedColor::Cursor => terminal_default_fg(),
            NamedColor::DimBlack => rgb_hex(0x76, 0x76, 0x76),
            NamedColor::DimRed => rgb_hex(0x9B, 0x0B, 0x15),
            NamedColor::DimGreen => rgb_hex(0x0E, 0x7A, 0x0A),
            NamedColor::DimYellow => rgb_hex(0x91, 0x75, 0x00),
            NamedColor::DimBlue => rgb_hex(0x00, 0x28, 0xA3),
            NamedColor::DimMagenta => rgb_hex(0x5C, 0x10, 0x6B),
            NamedColor::DimCyan => rgb_hex(0x2D, 0x70, 0xA3),
            NamedColor::DimWhite => rgb_hex(0x99, 0x99, 0x99),
            NamedColor::BrightForeground => rgb_hex(0xF2, 0xF2, 0xF2),
            NamedColor::DimForeground => rgb_hex(0x99, 0x99, 0x99),
        },
        Color::Spec(spec) => rgb_hex(spec.r, spec.g, spec.b),
        Color::Indexed(index) => ansi_index_to_rgb(index),
    }
}

fn ansi_index_to_rgb(index: u8) -> TerminalColor {
    match index {
        0 => rgb_hex(0x0C, 0x0C, 0x0C),
        1 => rgb_hex(0xC5, 0x0F, 0x1F),
        2 => rgb_hex(0x13, 0xA1, 0x0E),
        3 => rgb_hex(0xC1, 0x9C, 0x00),
        4 => rgb_hex(0x00, 0x37, 0xDA),
        5 => rgb_hex(0x88, 0x17, 0x98),
        6 => rgb_hex(0x3A, 0x96, 0xDD),
        7 => rgb_hex(0xCC, 0xCC, 0xCC),
        8 => rgb_hex(0x76, 0x76, 0x76),
        9 => rgb_hex(0xE7, 0x48, 0x56),
        10 => rgb_hex(0x16, 0xC6, 0x0C),
        11 => rgb_hex(0xF9, 0xF1, 0xA5),
        12 => rgb_hex(0x3B, 0x78, 0xFF),
        13 => rgb_hex(0xB4, 0x00, 0x9E),
        14 => rgb_hex(0x61, 0xD6, 0xD6),
        15 => rgb_hex(0xF2, 0xF2, 0xF2),
        16..=231 => {
            let index = index - 16;
            let r = index / 36;
            let g = (index % 36) / 6;
            let b = index % 6;
            let scale = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
            rgb_hex(scale(r), scale(g), scale(b))
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            rgb_hex(level, level, level)
        }
    }
}

struct GridSize {
    rows: usize,
    cols: usize,
}

impl Copy for GridSize {}
impl Clone for GridSize {
    fn clone(&self) -> Self {
        *self
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TerminalGrid {
    term: Term<VoidListener>,
    parser: Processor,
    size: GridSize,
}

impl TerminalGrid {
    pub fn new() -> Self {
        let size = GridSize {
            rows: 200,
            cols: 120,
        };
        let term = Term::new(Config::default(), &size, VoidListener);
        Self {
            term,
            parser: Processor::new(),
            size,
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.size = GridSize { rows, cols };
        self.term.resize(self.size);
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn render_colored_lines(&self) -> Vec<ColoredLine> {
        let mut lines = Vec::new();
        for row in 0..self.size.screen_lines() {
            let line = Line(row as i32);
            let mut spans: Vec<ColoredSpan> = Vec::new();
            let mut current_text = String::new();
            let mut current_fg = terminal_default_fg();
            let mut current_bg: Option<TerminalColor> = None;
            let mut current_bold = false;
            let mut current_dim = false;

            for col in 0..self.size.columns() {
                let cell = &self.term.grid()[line][Column(col)];
                let ch = cell.c;
                // Wide CJK characters occupy the cell plus its spacer. The
                // spacer must not be emitted as an extra space, or CJK output
                // drifts one column per character.
                if ch == ' '
                    && col > 0
                    && crate::selectable_text::is_wide_char(
                        self.term.grid()[line][Column(col - 1)].c,
                    )
                {
                    continue;
                }
                let fg = color_to_rgb(cell.fg);
                // `Background` means "no explicit bg" — keep transparent so the
                // theme `terminal_bg` (VSCode #1E1E1E / white) shows through.
                let bg = match cell.bg {
                    Color::Named(NamedColor::Background) => None,
                    other => {
                        let rgb = color_to_rgb(other);
                        // Alacritty default bg is black; treat pure-black bg as
                        // transparent for the same reason (PowerShell emits it).
                        if rgb == terminal_default_bg() {
                            None
                        } else {
                            Some(rgb)
                        }
                    }
                };
                let bold = cell.flags.contains(alacritty_terminal::term::cell::Flags::BOLD);
                let dim = cell
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::DIM);
                let same_style =
                    fg == current_fg && bg == current_bg && bold == current_bold && dim == current_dim;
                if current_text.is_empty() {
                    current_fg = fg;
                    current_bg = bg;
                    current_bold = bold;
                    current_dim = dim;
                    current_text.push(ch);
                } else if same_style {
                    current_text.push(ch);
                } else {
                    if !current_text.is_empty() {
                        spans.push(ColoredSpan {
                            text: current_text.clone(),
                            fg: current_fg,
                            bg: current_bg,
                            bold: current_bold,
                            dim: current_dim,
                        });
                    }
                    current_text = String::from(ch);
                    current_fg = fg;
                    current_bg = bg;
                    current_bold = bold;
                    current_dim = dim;
                }
            }
            if !current_text.is_empty() {
                spans.push(ColoredSpan {
                    text: current_text,
                    fg: current_fg,
                    bg: current_bg,
                    bold: current_bold,
                    dim: current_dim,
                });
            }
            let trimmed: String = spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("");
            if trimmed.trim().is_empty() {
                continue;
            }
            let mut trimmed_spans = spans;
            if let Some(last) = trimmed_spans.last_mut() {
                last.text = last.text.trim_end().to_string();
            }
            trimmed_spans.retain(|s| !s.text.is_empty());
            if !trimmed_spans.is_empty() {
                lines.push(ColoredLine {
                    spans: trimmed_spans,
                });
            }
        }
        lines
    }

    pub fn render_text(&self) -> String {
        let mut lines = Vec::new();
        for row in 0..self.size.screen_lines() {
            let line = Line(row as i32);
            let mut row_text = String::new();
            let mut trailing_spaces = 0usize;
            for col in 0..self.size.columns() {
                let cell = &self.term.grid()[line][Column(col)];
                let ch = cell.c;
                // Skip the spacer cell that follows a wide CJK character for
                // the same reason as in `render_colored_lines`.
                if ch == ' '
                    && col > 0
                    && crate::selectable_text::is_wide_char(
                        self.term.grid()[line][Column(col - 1)].c,
                    )
                {
                    continue;
                }
                if ch == ' ' {
                    trailing_spaces += 1;
                } else {
                    if trailing_spaces > 0 {
                        row_text.push_str(&" ".repeat(trailing_spaces));
                        trailing_spaces = 0;
                    }
                    row_text.push(ch);
                }
            }
            if row_text.is_empty() {
                continue;
            }
            lines.push(row_text.trim_end().to_string());
        }
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        lines.join("\n")
    }

    pub fn reset(&mut self) {
        let size = GridSize {
            rows: 200,
            cols: 120,
        };
        self.term = Term::new(Config::default(), &size, VoidListener);
        self.parser = Processor::new();
        self.size = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_converts_to_terminal_color() {
        assert_eq!(
            super::terminal_color_from_rgba(gpui::Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0
            }),
            super::TerminalColor(0xFFFFFF)
        );
        assert_eq!(
            super::terminal_color_from_rgba(gpui::Rgba {
                r: 0.2,
                g: 0.2,
                b: 0.2,
                a: 1.0
            }),
            super::TerminalColor(0x333333)
        );
    }

    #[test]
    fn renders_plain_text() {
        let mut grid = TerminalGrid::new();
        grid.write(b"hello\nworld");
        let text = grid.render_text();
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }

    #[test]
    fn handles_backspace() {
        let mut grid = TerminalGrid::new();
        grid.write(b"abc\x08");
        let text = grid.render_text();
        assert!(text.contains("ab"));
    }

    #[test]
    fn wide_char_spacer_cells_do_not_shift_columns() {
        // Alacritty stores a wide CJK char plus a spacer cell; rendering both
        // as text inserts one phantom space per CJK char and shifts columns.
        let mut grid = TerminalGrid::new();
        grid.write("ab中de".as_bytes());
        let text = grid.render_text();
        assert!(
            text.contains("ab中de"),
            "CJK spacer leaked into text: {text:?}"
        );
        let colored: String = grid
            .render_colored_lines()
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.text))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            colored.contains("ab中de"),
            "CJK spacer leaked into spans: {colored:?}"
        );
    }
}
