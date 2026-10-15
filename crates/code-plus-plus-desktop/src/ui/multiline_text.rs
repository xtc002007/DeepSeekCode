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

pub fn render_multiline(text: &str, style_line: impl Fn(Div) -> Div) -> impl IntoElement {
    // P2: empty text renders nothing (previously a ghost empty row).
    if text.is_empty() {
        return div().into_any_element();
    }
    let mut column = style_line(div().flex().flex_col().w_full().overflow_hidden());
    for line in split_display_lines(text) {
        // P2: long URLs/hashes must not blow out the container — keep the
        // line box bounded and ellipsis-truncated (GPUI has no break-all).
        column = column.child(
            style_line(div())
                .w_full()
                .max_w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(line),
        );
    }
    column.into_any_element()
}
