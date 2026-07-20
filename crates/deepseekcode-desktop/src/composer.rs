use deepseekcode_schema::provider::ProviderInfo;
use crate::context_usage::ContextUsageState;
use crate::theme::ThemeTokens;
use crate::ui::badge::badge_shell;
use crate::ui::button::{button_shell, icon_button_shell, ButtonSize, ButtonVariant};
use crate::ui::dropdown::DropdownOption;
use crate::ui::icon::{render_icon_rgba, IconName};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    prompt_input,
    [
        PromptBackspace,
        PromptDelete,
        PromptLeft,
        PromptRight,
        PromptSelectLeft,
        PromptSelectRight,
        PromptSelectAll,
        PromptHome,
        PromptEnd,
        PromptPaste,
        PromptCut,
        PromptCopy,
        PromptSubmit,
    ]
);

actions!(
    session_title_input,
    [SessionTitleSave, SessionTitleCancel]
);

#[derive(Clone, Debug)]
pub enum PromptInputEvent {
    PasteImage { bytes: Vec<u8>, format: ImageFormat },
}

pub struct PromptInput {
    focus_handle: FocusHandle,
    pub content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    caret_visible: bool,
    blink_generation: u64,
    masked: bool,
    compact: bool,
    multiline: bool,
    key_context_name: String,
}

impl PromptInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: "Type a prompt…".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            caret_visible: true,
            blink_generation: 0,
            masked: false,
            compact: false,
            multiline: false,
            key_context_name: "PromptInput".to_string(),
        }
    }

    pub fn set_multiline(&mut self, multiline: bool, cx: &mut Context<Self>) {
        self.multiline = multiline;
        cx.notify();
    }

    pub fn set_key_context(&mut self, name: impl Into<String>, cx: &mut Context<Self>) {
        self.key_context_name = name.into();
        cx.notify();
    }

    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        self.compact = compact;
        cx.notify();
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    fn display_text(&self) -> SharedString {
        if self.masked && !self.content.is_empty() {
            "•".repeat(self.content.chars().count()).into()
        } else if self.content.is_empty() {
            self.placeholder.clone()
        } else {
            // GPUI shape_line panics on newlines — always flatten for display.
            self.content.replace(['\r', '\n'], " ").into()
        }
    }

    fn normalize_inserted_text(&self, text: &str) -> String {
        if self.multiline {
            text.replace('\r', "")
        } else {
            text.replace(['\r', '\n'], " ")
        }
    }

    fn is_placeholder(&self) -> bool {
        self.content.is_empty()
    }

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink_generation = self.blink_generation.wrapping_add(1);
        let generation = self.blink_generation;
        cx.spawn(async move |this, cx| {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(530)).await;
                let still_current = this
                    .update(cx, |input, cx| {
                        if input.blink_generation != generation {
                            return false;
                        }
                        input.caret_visible = !input.caret_visible;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !still_current {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn text(&self) -> String {
        self.content.to_string()
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn left(&mut self, _: &PromptLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &PromptRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &PromptSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &PromptSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &PromptSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &PromptHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &PromptEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &PromptBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &PromptDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &PromptPaste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        let mut text_parts = Vec::new();
        for entry in clipboard.entries() {
            match entry {
                ClipboardEntry::String(text) => {
                    let s = text.text();
                    if !s.is_empty() {
                        text_parts.push(s.clone());
                    }
                }
                ClipboardEntry::Image(image) => {
                    cx.emit(PromptInputEvent::PasteImage {
                        bytes: image.bytes().to_vec(),
                        format: image.format(),
                    });
                }
                ClipboardEntry::ExternalPaths(_) => {}
            }
        }
        if !text_parts.is_empty() {
            self.replace_text_in_range(None, &text_parts.join(" "), window, cx);
        }
    }

    fn copy(&mut self, _: &PromptCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &PromptCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.restart_blink(cx);
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for PromptInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let new_text = self.normalize_inserted_text(new_text);

        self.content = (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
            .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.restart_blink(cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let new_text = self.normalize_inserted_text(new_text);

        self.content = (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
            .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct PromptTextElement {
    input: Entity<PromptInput>,
}

struct PromptPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl Element for PromptTextElement {
    type RequestLayoutState = ();
    type PrepaintState = PromptPrepaintState;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
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
        let input = self.input.read(cx);
        let display_text = input.display_text();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let theme = crate::theme::DeepSeekCodeTheme::tokens(cx);
        let text_color = if input.is_placeholder() {
            theme.text_muted.into()
        } else {
            theme.text_primary.into()
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len().saturating_sub(marked_range.end),
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    theme.text_primary,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x3a5a8acc),
                )),
                None,
            )
        };

        PromptPrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        let show_cursor = self.input.read(cx).caret_visible;
        if focus_handle.is_focused(window) && show_cursor {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl IntoElement for PromptTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Focusable for PromptInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PromptInputEvent> for PromptInput {}

impl Render for PromptInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let focused = self.focus_handle.is_focused(window);
        if focused && self.blink_generation == 0 {
            self.restart_blink(cx);
        } else if !focused {
            self.caret_visible = true;
        }

        div()
            .id("prompt-input")
            .flex_1()
            .when(!self.compact, |el| el.min_h(px(48.)))
            .when(self.compact, |el| el.h_full().min_h(px(0.)))
            .flex()
            .items_center()
            .when(self.compact, |el| el.px_2())
            .when(!self.compact, |el| el.p_2())
            .rounded_md()
            .when(!self.compact, |el| el.bg(theme.bg_elevated))
            .key_context(self.key_context_name.as_str())
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(if self.compact {
                crate::theme::leading_sm(&theme)
            } else {
                crate::theme::leading_base(&theme)
            })
            .text_size(if self.compact {
                theme.typography.text_sm
            } else {
                theme.typography.text_base
            })
            .child(
                div()
                    .w_full()
                    .min_h(if self.compact {
                        theme.typography.leading_sm
                    } else {
                        theme.typography.leading_base
                    })
                    .flex()
                    .items_center()
                    .child(PromptTextElement {
                        input: cx.entity().clone(),
                    }),
            )
    }
}

pub struct ComposerView;

#[derive(Clone, Debug)]
pub struct PromptAttachment {
    pub path: String,
    pub label: String,
}

impl ComposerView {
    pub fn render(
        cx: &mut Context<crate::workspace::WorkspaceView>,
        prompt_input: &Entity<PromptInput>,
        session_busy: bool,
        input_blocked: bool,
        agent_label: &str,
        model_label: &str,
        attachments: &[PromptAttachment],
        composer_dropdown: Option<&str>,
        composer_error: Option<&str>,
        has_session: bool,
        health_ok: bool,
        context_usage: &ContextUsageState,
    ) -> impl IntoElement {
        let theme = crate::theme::DeepSeekCodeTheme::tokens_from_context(cx);
        let send_icon = if session_busy {
            crate::ui::icon::IconName::Square
        } else {
            crate::ui::icon::IconName::Send
        };
        let send_bg = if session_busy {
            theme.error
        } else if input_blocked {
            theme.text_disabled
        } else {
            theme.accent_rgba()
        };

        let send_button = div()
            .id("prompt-send")
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.))
            .rounded_md()
            .bg(send_bg)
            .when(!input_blocked, |el| {
                el.cursor_pointer().hover(|s| {
                    s.bg(if session_busy {
                        theme.error
                    } else {
                        theme.accent_hover_rgba()
                    })
                })
            })
            .when(input_blocked, |el| el.opacity(0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if input_blocked {
                        return;
                    }
                    if this.session_busy {
                        this.interrupt_session(cx);
                    } else {
                        this.submit_prompt(cx);
                    }
                }),
            )
            .child(crate::ui::icon::render_icon_rgba(
                send_icon,
                px(16.),
                theme.text_on_accent,
            ));

        let agent_open = composer_dropdown == Some("agent");
        let model_open = composer_dropdown == Some("model");
        let accent_muted = theme.accent_muted;
        let border_focus = theme.border_focus;

        div()
            .id("composer")
            .w_full()
            .p_2()
            .bg(theme.bg_surface)
            .border_t_1()
            .border_color(theme.border_default)
            .flex()
            .flex_col()
            .gap(px(6.))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_external_paths(paths, cx);
            }))
            .drag_over(move |style, _: &ExternalPaths, _, _| {
                style
                    .bg(accent_muted)
                    .border_1()
                    .border_color(border_focus)
            })
            .children(if input_blocked {
                vec![div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(theme.warning.opacity(0.15))
                    .border_1()
                    .border_color(theme.warning.opacity(0.35))
                    .text_xs()
                    .text_color(theme.warning)
                    .child("Waiting for permission or question approval…")
                    .into_any_element()]
            } else {
                vec![]
            })
            .children(if let Some(error) = composer_error {
                vec![div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.error.opacity(0.15))
                    .border_1()
                    .border_color(theme.error.opacity(0.4))
                    .text_xs()
                    .text_color(theme.error)
                    .child(error.to_string())
                    .into_any_element()]
            } else {
                vec![]
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .items_center()
                    .child(
                        icon_button_shell(
                            "attach-file",
                            IconName::Paperclip,
                            &theme,
                            px(32.),
                            false,
                            false,
                            false,
                        )
                        .rounded_md()
                        .bg(theme.bg_elevated)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.attach_file(cx);
                        })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(36.))
                            .max_h(px(72.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .border_1()
                            .border_color(theme.border_subtle)
                            .rounded_md()
                            .bg(theme.bg_elevated)
                            .children(attachments.iter().enumerate().map(|(idx, att)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_2()
                                    .py_0p5()
                                    .mx_1()
                                    .mt_1()
                                    .rounded_md()
                                    .bg(theme.bg_active)
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(att.label.clone())
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, {
                                                cx.listener(move |this, _, _, cx| {
                                                    this.remove_attachment(idx, cx);
                                                })
                                            })
                                            .child(crate::ui::icon::render_icon_rgba(
                                                crate::ui::icon::IconName::X,
                                                px(12.),
                                                theme.error,
                                            )),
                                    )
                                    .into_any_element()
                            }))
                            .when(input_blocked, |el| el.opacity(0.5))
                            .child(prompt_input.clone()),
                    )
                    .child(send_button),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .flex_wrap()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .flex_wrap()
                            .child(badge_dropdown(
                                cx,
                                "agent",
                                agent_label,
                                agent_open,
                                &theme,
                                IconName::Bot,
                            ))
                            .child(badge_dropdown(
                                cx,
                                "model",
                                model_label,
                                model_open,
                                &theme,
                                IconName::Sparkles,
                            )),
                    )
                    .children(if has_session {
                        vec![div()
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_xs()
                            .line_height(crate::theme::leading_xs(&theme))
                            .text_color(theme.text_muted)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .w(px(6.))
                                            .h(px(6.))
                                            .rounded_full()
                                            .bg(if health_ok {
                                                theme.success
                                            } else {
                                                theme.error
                                            }),
                                    )
                                    .child(if health_ok { "Local" } else { "Offline" }),
                            )
                            .child(crate::context_usage::ContextUsageMeter::render_compact(
                                cx,
                                context_usage,
                            ))
                            .child(
                                button_shell(
                                    "compact-session",
                                    "Compact",
                                    &theme,
                                    ButtonVariant::Ghost,
                                    ButtonSize::Sm,
                                    false,
                                    None,
                                    false,
                                )
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.compact_current_session(cx);
                                })),
                            )
                            .child(
                                button_shell(
                                    "session-menu",
                                    "更多",
                                    &theme,
                                    ButtonVariant::Secondary,
                                    ButtonSize::Sm,
                                    false,
                                    Some(IconName::MoreHorizontal),
                                    false,
                                )
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.toggle_session_menu_at(event.position, cx);
                                })),
                            )
                            .children(if context_usage.expanded {
                                vec![crate::context_usage::ContextUsageMeter::render_detail_popover(
                                    context_usage,
                                    &theme,
                                )
                                .into_any_element()]
                            } else {
                                vec![]
                            })
                            .into_any_element()]
                    } else {
                        vec![]
                    }),
            )
    }
}

fn badge_dropdown(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    kind: &'static str,
    label: &str,
    open: bool,
    theme: &ThemeTokens,
    icon: IconName,
) -> impl IntoElement {
    badge_shell(
        ElementId::Name(SharedString::from(format!("composer-{kind}"))),
        label.to_string(),
        theme,
        Some(icon),
        open,
        true,
    )
    .on_mouse_down(MouseButton::Left, {
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.toggle_composer_dropdown(kind, event.position, cx);
        })
    })
}

pub fn agent_dropdown_options() -> Vec<DropdownOption> {
    ["build", "plan", "general"]
        .iter()
        .map(|a| DropdownOption {
            id: (*a).into(),
            label: (*a).into(),
            disabled: false,
        })
        .collect()
}

pub fn model_dropdown_options(providers: &[ProviderInfo]) -> Vec<DropdownOption> {
    providers
        .iter()
        .flat_map(|p| {
            p.models.iter().map(move |m| DropdownOption {
                id: format!("{}:{}", p.id, m.id),
                label: format!("{} — {}", p.name, m.name),
                disabled: !p.connected,
            })
        })
        .collect()
}

pub fn composer_dropdown_selected_id(
    kind: &str,
    agent_label: &str,
    model_label: &str,
    providers: &[ProviderInfo],
) -> String {
    match kind {
        "agent" => agent_label.to_string(),
        "model" => {
            let _ = providers;
            model_label.to_string()
        }
        _ => String::new(),
    }
}

pub fn bind_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", PromptBackspace, Some("PromptInput")),
        KeyBinding::new("delete", PromptDelete, Some("PromptInput")),
        KeyBinding::new("left", PromptLeft, Some("PromptInput")),
        KeyBinding::new("right", PromptRight, Some("PromptInput")),
        KeyBinding::new("shift-left", PromptSelectLeft, Some("PromptInput")),
        KeyBinding::new("shift-right", PromptSelectRight, Some("PromptInput")),
        KeyBinding::new("ctrl-a", PromptSelectAll, Some("PromptInput")),
        KeyBinding::new("ctrl-v", PromptPaste, Some("PromptInput")),
        KeyBinding::new("paste", PromptPaste, Some("PromptInput")),
        KeyBinding::new("shift-insert", PromptPaste, Some("PromptInput")),
        KeyBinding::new("ctrl-c", PromptCopy, Some("PromptInput")),
        KeyBinding::new("ctrl-x", PromptCut, Some("PromptInput")),
        KeyBinding::new("home", PromptHome, Some("PromptInput")),
        KeyBinding::new("end", PromptEnd, Some("PromptInput")),
        KeyBinding::new("enter", PromptSubmit, Some("PromptInput")),
        KeyBinding::new("enter", SessionTitleSave, Some("SessionTitleInput")),
        KeyBinding::new("escape", SessionTitleCancel, Some("SessionTitleInput")),
    ]);
}
