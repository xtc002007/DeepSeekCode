use crate::ui::multiline_text::split_display_lines;
use gpui::*;

pub struct FileEditorInput {
    focus_handle: FocusHandle,
    pub content: String,
}

impl FileEditorInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
        }
    }

    pub fn text(&self) -> String {
        self.content.clone()
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        cx.notify();
    }

    fn insert_str(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.content.push_str(text);
        cx.notify();
    }

    fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        if let Some(key_char) = &keystroke.key_char {
            if key_char == "\u{8}" || key_char == "\u{7f}" {
                self.content.pop();
                cx.notify();
                return;
            }
            if key_char == "\r" || key_char == "\n" {
                self.content.push('\n');
                cx.notify();
                return;
            }
            if key_char.chars().all(|c| !c.is_control() || c == '\t') {
                self.insert_str(key_char, cx);
            }
            return;
        }
        match keystroke.key.as_str() {
            "backspace" => {
                self.content.pop();
                cx.notify();
            }
            "enter" => {
                self.content.push('\n');
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Focusable for FileEditorInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileEditorInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let lines = split_display_lines(&self.content);

        let mut body = div().flex().flex_col().gap_0p5().w_full();
        for (idx, line) in lines.iter().enumerate() {
            let line_no = idx + 1;
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
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .text_color(theme.text_primary)
                            .child(if line.is_empty() && lines.len() == 1 && self.content.is_empty() {
                                "(empty file)".to_string()
                            } else {
                                line.clone()
                            }),
                    ),
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
