//! GPUI text nodes cannot contain embedded newlines — render one line per child.

use gpui::*;

pub fn flatten_line(text: &str) -> String {
    text.replace('\r', " ").replace('\n', " ")
}

pub fn split_display_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

pub fn render_multiline(
    text: &str,
    style_line: impl Fn(Div) -> Div,
) -> impl IntoElement {
    let mut column = style_line(div().flex().flex_col().w_full());
    for line in split_display_lines(text) {
        column = column.child(style_line(div()).child(line));
    }
    column
}
