use crate::ui::multiline_text::split_display_lines;
use gpui::*;

pub enum FileEditorInputEvent {
    Edited,
    SaveRequested,
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

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // P0-3: click focuses the editor; previously clicks did nothing so
        // typing after a click felt like a dead key.
        window.focus(&self.focus_handle, cx);
        // Best-effort line placement: each rendered line is 20px tall.
        // Without text-layout measurement we map y -> line index and place
        // the cursor at that line start. x-precise column mapping would need
        // shaped-text measurement which GPUI does not expose here.
        let lines = split_display_lines(&self.content);
        if !lines.is_empty() {
            let line_idx = ((event.position.y / px(20.)).floor() as usize).min(lines.len() - 1);
            let mut byte_pos = 0usize;
            for (idx, line) in lines.iter().enumerate() {
                if idx == line_idx {
                    self.cursor = byte_pos.min(self.content.len());
                    break;
                }
                byte_pos += line.len() + 1;
            }
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let pos = self.cursor_offset();
        if pos >= self.content.len() {
            return;
        }
        let next = self.content[pos..]
            .char_indices()
            .nth(1)
            .map(|(off, _)| pos + off)
            .unwrap_or(self.content.len());
        self.content.replace_range(pos..next, "");
        cx.emit(FileEditorInputEvent::Edited);
        cx.notify();
    }

    fn move_line(&mut self, delta: isize, cx: &mut Context<Self>) {
        let lines = split_display_lines(&self.content);
        if lines.is_empty() {
            return;
        }
        let cursor = self.cursor_offset();
        let mut byte_pos = 0usize;
        let mut cur_line = 0usize;
        let mut cur_col = 0usize;
        for (idx, line) in lines.iter().enumerate() {
            let end = byte_pos + line.len();
            if cursor <= end || idx == lines.len() - 1 {
                cur_line = idx;
                cur_col = cursor.saturating_sub(byte_pos);
                break;
            }
            byte_pos = end + 1;
        }
        let target = (cur_line as isize + delta).clamp(0, lines.len() as isize - 1) as usize;
        if target == cur_line {
            return;
        }
        let mut pos = 0usize;
        for (idx, line) in lines.iter().enumerate() {
            if idx == target {
                let col = cur_col.min(line.len());
                self.cursor = (pos + col).min(self.content.len());
                cx.notify();
                return;
            }
            pos += line.len() + 1;
        }
    }

    fn move_home(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let line_start = self.content[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.cursor = line_start;
        cx.notify();
    }

    fn move_end(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let rel = self.content[cursor..].find('\n').map(|i| cursor + i).unwrap_or(self.content.len());
        self.cursor = rel;
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            // P0-4: previously every ctrl/cmd combo except Save was swallowed,
            // so copy/paste/select-all were dead keys. Handle the full
            // clipboard set here (FileEditorInput uses on_key_down directly,
            // not the PromptInput action map).
            match keystroke.key.as_str() {
                "s" => {
                    cx.emit(FileEditorInputEvent::SaveRequested);
                    cx.notify();
                }
                "a" => {
                    // Select-all surrogate: jump to end so a subsequent
                    // ctrl+c copies the whole buffer (no range state yet).
                    self.cursor = self.content.len();
                    cx.notify();
                }
                "c" => {
                    if !self.content.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(self.content.clone()));
                    }
                }
                "x" => {
                    if !self.content.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(self.content.clone()));
                        self.content.clear();
                        self.cursor = 0;
                        cx.emit(FileEditorInputEvent::Edited);
                        cx.notify();
                    }
                }
                "v" => {
                    if let Some(clip) = cx.read_from_clipboard() {
                        let mut parts = Vec::new();
                        for entry in clip.entries() {
                            if let ClipboardEntry::String(s) = entry {
                                let t = s.text();
                                if !t.is_empty() {
                                    parts.push(t.clone());
                                }
                            }
                        }
                        if !parts.is_empty() {
                            // P0-4: keep newlines (composer joins with space,
                            // editor must preserve them).
                            self.insert_str(&parts.join("\n"), cx);
                        }
                    }
                }
                _ => {
                    window.play_system_bell();
                }
            }
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
            "delete" => self.delete_forward(cx),
            "enter" => self.insert_str("\n", cx),
            "up" => self.move_line(-1, cx),
            "down" => self.move_line(1, cx),
            "home" => self.move_home(cx),
            "end" => self.move_end(cx),
            "pageup" => self.move_line(-10, cx),
            "pagedown" => self.move_line(10, cx),
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
        let theme = crate::theme::CodePlusPlusTheme::tokens_from_context(cx);
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

        // Same gutter metrics as the read-only view (`render_line_gutter`):
        // shared width formula, structural right alignment, so numbers line
        // up in every case (1/2/3-digit files, editable or read-only).
        let gutter_width = crate::editor::line_gutter_width(lines.len());
        let mut body = div().flex().flex_col().gap_0p5().w_full();
        for (idx, line) in lines.iter().enumerate() {
            let line_no = idx + 1;
            let is_cursor_line = idx == cursor_line;
            body = body.child(
                div()
                    .flex()
                    .text_sm()
                    .line_height(px(20.))
                    .font_family(crate::theme::font_mono(&theme))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .flex_shrink_0()
                            .w(gutter_width)
                            .px_2()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(theme.text_disabled)
                            .child(format!("{line_no}")),
                    )
                    .child(div().flex_1().px_2().text_color(theme.text_primary).child(
                        if line.is_empty() && lines.len() == 1 && self.content.is_empty() {
                            "(empty file)".to_string()
                        } else if is_cursor_line {
                            // P0-3: byte-safe split — cursor_col is a byte
                            // offset and may land inside a multi-byte grapheme
                            // (emoji); clamp to a char boundary to avoid panic.
                            let mut col = cursor_col.min(line.len());
                            while col > 0 && !line.is_char_boundary(col) {
                                col -= 1;
                            }
                            let (before, after) = line.split_at(col);
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(body)
    }
}
