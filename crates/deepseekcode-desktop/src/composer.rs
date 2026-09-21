use crate::context_usage::ContextUsageState;
use crate::theme::ThemeTokens;
use crate::ui::badge::badge_shell;
use crate::ui::button::{button_shell, icon_button_shell, ButtonSize, ButtonVariant};
use crate::ui::dropdown::DropdownOption;
use crate::ui::icon::{render_icon_rgba, IconName};
use deepseekcode_schema::provider::ProviderInfo;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

pub const COMPOSER_PLACEHOLDER: &str = "Ask anything, @ to mention, / for actions";

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

actions!(session_title_input, [SessionTitleSave, SessionTitleCancel]);

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
    small_text: bool,
    multiline: bool,
    key_context_name: String,
}

impl PromptInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: COMPOSER_PLACEHOLDER.into(),
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
            small_text: false,
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

    /// Render with `text_sm` instead of `text_base` (composer input matches
    /// surrounding UI density without affecting dialog inputs).
    pub fn set_small_text(&mut self, small: bool, cx: &mut Context<Self>) {
        self.small_text = small;
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
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
        cx.spawn(async move |this, cx| loop {
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
        })
        .detach();
    }

    pub fn focus_handle_clone(&self) -> FocusHandle {
        self.focus_handle.clone()
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

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
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

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
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
            .line_height(if self.compact || self.small_text {
                crate::theme::leading_sm(&theme)
            } else {
                crate::theme::leading_base(&theme)
            })
            .text_size(if self.compact || self.small_text {
                theme.typography.text_sm
            } else {
                theme.typography.text_base
            })
            .child(
                div()
                    .w_full()
                    .min_h(if self.compact || self.small_text {
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

impl PromptAttachment {
    pub fn file_name(&self) -> String {
        std::path::Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.clone())
    }

    pub fn is_image(&self) -> bool {
        is_image_path(&self.path)
    }
}

pub fn is_image_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico"
            )
        })
        .unwrap_or(false)
}

fn image_format_for_path(path: &str) -> ImageFormat {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        _ => ImageFormat::Png,
    }
}

/// Load an attachment image from disk (thumbnails + preview share this).
/// Returns `None` when the file is missing, too large, or undecodable.
/// The cap admits uncompressed BMP screenshots up to ~4K.
pub fn load_attachment_image(path: &str) -> Option<Arc<Image>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
        return None;
    }
    let format = image_format_for_path(path);
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// Normalize pasted clipboard bytes to BMP so pastes always land as
/// `paste-<timestamp>.bmp`, no matter which clipboard format (PNG/JPG/DIB…)
/// the OS enumerated first. Returns `None` for undecodable bytes
/// (e.g. SVG, which the raster decoder cannot handle).
pub fn encode_clipboard_image_as_bmp(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgb = img.to_rgb8();
    let mut bmp = Vec::new();
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
        .ok()?;
    if bmp.is_empty() {
        return None;
    }
    Some(bmp)
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
            crate::ui::icon::IconName::ArrowRight
        };
        let send_bg = if session_busy {
            theme.error
        } else if input_blocked {
            theme.text_disabled
        } else {
            Rgba {
                r: 0x1a as f32 / 255.0,
                g: 0x73 as f32 / 255.0,
                b: 0xe8 as f32 / 255.0,
                a: 1.0,
            }
        };

        let send_button = div()
            .id("prompt-send")
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded_full()
            .bg(send_bg)
            .when(!input_blocked, |el| {
                el.cursor_pointer().hover(|s| {
                    s.bg(if session_busy {
                        theme.error
                    } else {
                        Rgba {
                            r: 0x17 as f32 / 255.0,
                            g: 0x65 as f32 / 255.0,
                            b: 0xcc as f32 / 255.0,
                            a: 1.0,
                        }
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
                px(14.),
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
                style.bg(accent_muted).border_1().border_color(border_focus)
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
                    .w_full()
                    .rounded_lg()
                    .bg(theme.bg_elevated)
                    .border_1()
                    .border_color(theme.border_subtle)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    // ——— attachment strip: image thumbnails (click = preview) + file chips ———
                    .when(!attachments.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(6.))
                                .px_2()
                                .pt_1()
                                .children(attachments.iter().enumerate().map(|(idx, att)| {
                                    if att.is_image() {
                                        let thumb: AnyElement =
                                            match load_attachment_image(&att.path) {
                                                Some(image) => div()
                                                    .w_full()
                                                    .h_full()
                                                    .overflow_hidden()
                                                    .rounded_md()
                                                    .child(
                                                        img(image).w_full().h_full().rounded_md(),
                                                    )
                                                    .into_any_element(),
                                                None => div()
                                                    .w_full()
                                                    .h_full()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded_md()
                                                    .bg(theme.bg_active)
                                                    .child(render_icon_rgba(
                                                        IconName::File,
                                                        px(16.),
                                                        theme.text_muted,
                                                    ))
                                                    .into_any_element(),
                                            };
                                        div()
                                            .id(ElementId::Name(SharedString::from(format!(
                                                "att-thumb-{idx}"
                                            ))))
                                            .relative()
                                            .w(px(40.))
                                            .h(px(40.))
                                            .rounded_md()
                                            .overflow_hidden()
                                            .border_1()
                                            .border_color(theme.border_subtle)
                                            .bg(theme.bg_surface)
                                            .cursor_pointer()
                                            .hover(|s| s.border_color(theme.border_focus))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.open_image_preview(idx, cx);
                                                }),
                                            )
                                            .child(thumb)
                                            .child(
                                                div()
                                                    .absolute()
                                                    .top(px(2.))
                                                    .right(px(2.))
                                                    .size(px(14.))
                                                    .rounded_full()
                                                    .bg(rgba(0x000000cc))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .on_mouse_down(MouseButton::Left, {
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.remove_attachment(idx, cx);
                                                        })
                                                    })
                                                    .child(render_icon_rgba(
                                                        IconName::X,
                                                        px(10.),
                                                        Rgba {
                                                            r: 1.0,
                                                            g: 1.0,
                                                            b: 1.0,
                                                            a: 1.0,
                                                        },
                                                    )),
                                            )
                                            .into_any_element()
                                    } else {
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(6.))
                                            .px_2()
                                            .py_0p5()
                                            .rounded_md()
                                            .bg(theme.bg_active)
                                            .border_1()
                                            .border_color(theme.border_subtle)
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(render_icon_rgba(
                                                        IconName::File,
                                                        px(12.),
                                                        theme.text_muted,
                                                    ))
                                                    .child(att.file_name()),
                                            )
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
                                                    .child(render_icon_rgba(
                                                        IconName::X,
                                                        px(12.),
                                                        theme.error,
                                                    )),
                                            )
                                            .into_any_element()
                                    }
                                })),
                        )
                    })
                    // ——— text input ———
                    .child(
                        div()
                            .w_full()
                            .min_h(px(34.))
                            .px_2()
                            .pt_1()
                            .pb_1()
                            .when(input_blocked, |el| el.opacity(0.5))
                            .child(prompt_input.clone()),
                    )
                    // ——— divider ———
                    .child(div().w_full().h(px(1.)).mx_2().bg(theme.border_subtle))
                    // ——— bottom toolbar: [+] [model ^] … [mic] [send] ———
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(6.))
                            .px_2()
                            .py_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        icon_button_shell(
                                            "composer-add",
                                            IconName::Plus,
                                            &theme,
                                            px(24.),
                                            false,
                                            false,
                                            false,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.attach_file(cx);
                                            }),
                                        ),
                                    )
                                    .child(model_picker_pill(cx, model_label, model_open, &theme)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        icon_button_shell(
                                            "composer-mic",
                                            IconName::Mic,
                                            &theme,
                                            px(24.),
                                            false,
                                            false,
                                            false,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, window, cx| {
                                                this.focus_prompt_input(window, cx);
                                            }),
                                        ),
                                    )
                                    .child(send_button),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .flex_wrap()
                    .child(div().flex().items_center().gap(px(6.)).flex_wrap().child(
                        badge_dropdown(cx, "agent", agent_label, agent_open, &theme, IconName::Bot),
                    ))
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
                                    .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(
                                        if health_ok {
                                            theme.success
                                        } else {
                                            theme.error
                                        },
                                    ))
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
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.compact_current_session(cx);
                                    }),
                                ),
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
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                        this.toggle_session_menu_at(event.position, cx);
                                    }),
                                ),
                            )
                            .children(if context_usage.expanded {
                                vec![
                                    crate::context_usage::ContextUsageMeter::render_detail_popover(
                                        context_usage,
                                        &theme,
                                    )
                                    .into_any_element(),
                                ]
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

/// Screenshot-style model pill: `<model label> ^`, opens the model dropdown.
fn model_picker_pill(
    cx: &mut Context<crate::workspace::WorkspaceView>,
    label: &str,
    open: bool,
    theme: &ThemeTokens,
) -> impl IntoElement {
    div()
        .id("composer-model-pill")
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_sm()
        .text_color(theme.text_secondary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .when(open, |s| s.bg(theme.bg_active))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.toggle_composer_dropdown("model", event.position, cx);
            }),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label.to_string()),
        )
        .child(render_icon_rgba(
            if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronUp
            },
            px(12.),
            theme.text_muted,
        ))
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
