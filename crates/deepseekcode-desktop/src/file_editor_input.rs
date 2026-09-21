use crate::ui::multiline_text::split_display_lines;
use gpui::*;

pub enum FileEditorInputEvent {
    Edited,
}

pub struct FileEditorInput {
    focus_handle: FocusHandle,
    pub content: String,
    cursor: usize,
}

impl FileEditorInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            cursor: 0,
        }
    }

    pub fn text(&self) -> String {
        self.content.clone()
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.cursor = self.content.len();
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        self.cursor.min(self.content.len())
    }

    fn insert_str(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let pos = self.cursor_offset();
        self.content.insert_str(pos, text);
        self.cursor = pos + text.len();
        cx.emit(FileEditorInputEvent::Edited);
        cx.notify();
    }

    fn delete_before_cursor(&mut self, cx: &mut Context<Self>) {
        let pos = self.cursor_offset();
        if pos == 0 {
            return;
        }
        let prev = self.content[..pos]
            .char_indices()
            .next_back()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        self.content.replace_range(prev..pos, "");
        self.cursor = prev;
        cx.emit(FileEditorInputEvent::Edited);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        if let Some(key_char) = &keystroke.key_char {
            if key_char == "\u{8}" || key_char == "\u{7f}" {
                self.delete_before_cursor(cx);
                return;
            }
            if key_char == "\r" || key_char == "\n" {
                self.insert_str("\n", cx);
                return;
            }
            if key_char.chars().all(|c| !c.is_control() || c == '\t') {
                self.insert_str(key_char, cx);
            }
            return;
        }
        match keystroke.key.as_str() {
            "backspace" => self.delete_before_cursor(cx),
            "enter" => self.insert_str("\n", cx),
            "left" => {
                let pos = self.cursor_offset();
                if pos > 0 {
                    self.cursor = self.content[..pos]
                        .char_indices()
                        .next_back()
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    cx.notify();
                }
            }
            "right" => {
                let pos = self.cursor_offset();
                if pos < self.content.len() {
                    self.cursor = self.content[pos..]
                        .char_indices()
                        .nth(1)
                        .map(|(offset, _)| pos + offset)
                        .unwrap_or(self.content.len());
                    cx.notify();
                }
            }
            _ => {}
        }
    }
}

impl EventEmitter<FileEditorInputEvent> for FileEditorInput {}

impl Focusable for FileEditorInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileEditorInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let lines = split_display_lines(&self.content);
        let cursor = self.cursor_offset();
        let mut byte_pos = 0usize;
        let mut cursor_line = 0usize;
        let mut cursor_col = 0usize;
        for (idx, line) in lines.iter().enumerate() {
            let line_end = byte_pos + line.len();
            if cursor <= line_end || (idx == lines.len() - 1 && cursor >= byte_pos) {
                cursor_line = idx;
                cursor_col = cursor.saturating_sub(byte_pos);
                break;
            }
            byte_pos = line_end + 1;
        }

        let mut body = div().flex().flex_col().gap_0p5().w_full();
        for (idx, line) in lines.iter().enumerate() {
            let line_no = idx + 1;
            let is_cursor_line = idx == cursor_line;
            body = body.child(
                div()
                    .flex()
                    .text_sm()
                    .font_family(crate::theme::font_mono(&theme))
                    .child(
                        div()
                            .w(px(48.))
                            .px_2()
                            .text_color(theme.text_disabled)
                            .text_align(TextAlign::Right)
                            .child(format!("{line_no}")),
                    )
                    .child(div().flex_1().px_2().text_color(theme.text_primary).child(
                        if line.is_empty() && lines.len() == 1 && self.content.is_empty() {
                            "(empty file)".to_string()
                        } else if is_cursor_line && cursor_col <= line.len() {
                            let (before, after) = line.split_at(cursor_col);
                            format!("{before}|{after}")
                        } else {
                            line.clone()
                        },
                    )),
            );
        }

        div()
            .id("file-editor-input")
            .w_full()
            .h_full()
            .key_context("FileEditorInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(body)
    }
}
