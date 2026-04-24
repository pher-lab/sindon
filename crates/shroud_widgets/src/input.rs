//! Input widget — standard text input for non-sensitive data.
//!
//! Unlike `SecureInput`, the value is a plain `String` and rendered as-is
//! (no masking). Supports cursor movement, Home/End, Delete, and
//! `on_change` / `on_submit` callbacks.
//!
//! ## Reactive value binding (Phase 18d)
//!
//! Callers can bind the input to a `Signal<String>` via
//! [`Input::value`]. The binding is bidirectional: external writes
//! (`signal.set("...")`) are picked up on the next paint (buffer rebases,
//! cursor clamps), and every keystroke writes the fresh buffer back to the
//! signal. No Effect loop — widgets don't subscribe to signals, they just
//! pull on paint; a write-back to the same signal has no subscribers that
//! would re-trigger this widget.

use std::cell::{Cell, RefCell};

use crate::event::{EventContext, EventResult, Key, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::Signal;

/// A standard text input field.
///
/// # Example (conceptual)
/// ```ignore
/// let text = Signal::new(String::new());
/// let input = Input::new()
///     .placeholder("Enter username")
///     .value(text)
///     .on_submit(|s, _ctx| println!("submitted: {s}"));
/// ```
type TextCallback = Box<dyn FnMut(&str, &mut EventContext)>;

pub struct Input {
    /// Editable buffer. `RefCell` because [`Widget::paint`] takes `&self`
    /// but may need to rebase the buffer from the bound `Signal<String>`.
    value: RefCell<String>,
    /// Cursor byte offset into `value`. `Cell` for the same reason —
    /// paint-time sync may have to clamp it when the external signal
    /// produces a shorter string than the local buffer.
    cursor: Cell<usize>,
    /// Optional external binding. When `Some`, every paint and event
    /// rebases `value` from the signal if they differ, and every edit
    /// writes the fresh buffer back.
    source: Option<Signal<String>>,
    placeholder: String,
    font_size: Option<f32>,
    focused: bool,
    on_change: Option<TextCallback>,
    on_submit: Option<TextCallback>,
    // Colors (None = read from theme)
    bg_color: Option<Color>,
    bg_focused_color: Option<Color>,
    text_color: Option<Color>,
    placeholder_color: Option<Color>,
    border_color: Option<Color>,
    border_focused_color: Option<Color>,
}

impl Input {
    /// Create a new empty input.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(String::new()),
            cursor: Cell::new(0),
            source: None,
            placeholder: String::new(),
            font_size: None,
            focused: false,
            on_change: None,
            on_submit: None,
            bg_color: None,
            bg_focused_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            border_focused_color: None,
        }
    }

    /// Create an input with initial value.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let v = value.into();
        self.cursor.set(v.len());
        self.value = RefCell::new(v);
        self
    }

    /// Bind this input to a `Signal<String>` (bidirectional).
    ///
    /// On every paint and event, the widget rebases its buffer from the
    /// signal if they differ (clamping the cursor). On every edit, the
    /// widget writes the fresh buffer back to the signal. Callers can
    /// therefore read the current text via `signal.get_clone()` and clear
    /// the input with `signal.set(String::new())` — without needing a
    /// subtree rebuild.
    ///
    /// The initial buffer seeds from the signal's current value.
    pub fn value(mut self, signal: Signal<String>) -> Self {
        let initial = signal.get_clone();
        self.cursor.set(initial.len());
        *self.value.borrow_mut() = initial;
        self.source = Some(signal);
        self
    }

    /// Set the placeholder text.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set a callback for when the text changes.
    ///
    /// Receives the current value and the [`EventContext`]. When the input
    /// is bound via [`Input::value`], `on_change` fires *after* the bound
    /// signal is updated, so handlers can read the same text from either
    /// source.
    pub fn on_change(mut self, f: impl FnMut(&str, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Set a callback for when Enter is pressed.
    ///
    /// Receives the current value and the [`EventContext`] — handlers can
    /// drive screen transitions (`ctx.replace_screen(...)`) in response.
    pub fn on_submit(mut self, f: impl FnMut(&str, &mut EventContext) + 'static) -> Self {
        self.on_submit = Some(Box::new(f));
        self
    }

    /// Set the background color.
    pub fn background(mut self, color: Color) -> Self {
        self.bg_color = Some(color);
        self
    }

    /// Set the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Get a clone of the current value.
    pub fn value_clone(&self) -> String {
        self.value.borrow().clone()
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.value.borrow().is_empty()
    }

    /// Whether this input currently has focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Get cursor position (byte offset).
    pub fn cursor(&self) -> usize {
        self.cursor.get()
    }

    fn resolve_bg(&self, colors: &shroud_core::Colors) -> Color {
        if self.focused {
            self.bg_focused_color
                .unwrap_or(colors.input_background_focused)
        } else {
            self.bg_color.unwrap_or(colors.input_background)
        }
    }

    fn resolve_border(&self, colors: &shroud_core::Colors) -> Color {
        if self.focused {
            self.border_focused_color
                .unwrap_or(colors.input_border_focused)
        } else {
            self.border_color.unwrap_or(colors.input_border)
        }
    }

    /// Find the previous char boundary before `pos` in `s`.
    fn prev_char_boundary(s: &str, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut i = pos - 1;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Find the next char boundary after `pos` in `s`.
    fn next_char_boundary(s: &str, pos: usize) -> usize {
        if pos >= s.len() {
            return s.len();
        }
        let mut i = pos + 1;
        while i < s.len() && !s.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    /// Rebase the buffer from the bound signal if they differ. Clamps the
    /// cursor to the new length. Called from both `paint` (via `&self`,
    /// interior-mutable) and `event` (via `&mut self`, same path).
    fn sync_from_source(&self) {
        if let Some(src) = self.source.as_ref() {
            let remote = src.get_clone();
            let mut buf = self.value.borrow_mut();
            if *buf != remote {
                let new_len = remote.len();
                *buf = remote;
                if self.cursor.get() > new_len {
                    self.cursor.set(new_len);
                }
            }
        }
    }

    /// Push the current buffer back to the bound signal (if any). Called
    /// after each edit.
    fn push_to_source(&self) {
        if let Some(src) = self.source.as_ref() {
            src.set(self.value.borrow().clone());
        }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Input {
    fn focusable(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new().padding(8.0).min_height(font_size + 20.0)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        self.sync_from_source();

        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let text_color = self.text_color.unwrap_or(ctx.theme.colors.on_surface);
        let placeholder_color = self
            .placeholder_color
            .unwrap_or(ctx.theme.colors.input_placeholder);
        let bg = self.resolve_bg(&ctx.theme.colors);
        let border = self.resolve_border(&ctx.theme.colors);

        // Background
        ctx.fill_rect(layout, bg);

        // Border (1px)
        let b = 1.0;
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.bottom() - b, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, b, layout.size.height),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.right() - b, layout.origin.y, b, layout.size.height),
            border,
        );

        let text_x = layout.origin.x + 8.0;
        let text_y = layout.origin.y + (layout.size.height - font_size) / 2.0;
        let max_width = layout.size.width - 16.0;

        let value = self.value.borrow();
        if value.is_empty() {
            // Placeholder
            if !self.placeholder.is_empty() {
                let shaped = ctx.text_engine.shape_text(
                    &self.placeholder,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );
                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            text_x as i32 + glyph.x,
                            text_y as i32 + glyph.y,
                            image,
                            placeholder_color,
                            glyph.cache_key,
                        );
                    }
                }
            }

            // Cursor at start when focused and empty
            if self.focused {
                ctx.fill_rect(Rect::new(text_x, text_y, 2.0, font_size), text_color);
            }
        } else {
            // Render actual text
            let shaped =
                ctx.text_engine
                    .shape_text(&value, font_size, font_size * 1.2, Some(max_width));

            for glyph in &shaped.glyphs {
                if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                    ctx.draw_glyph(
                        text_x as i32 + glyph.x,
                        text_y as i32 + glyph.y,
                        image,
                        text_color,
                        glyph.cache_key,
                    );
                }
            }

            // Cursor
            if self.focused {
                let cursor = self.cursor.get();
                let cursor_x = if cursor == 0 {
                    text_x
                } else {
                    let before_cursor = &value[..cursor];
                    let shaped_before =
                        ctx.text_engine
                            .shape_text(before_cursor, font_size, font_size * 1.2, None);
                    text_x + shaped_before.width
                };
                ctx.fill_rect(Rect::new(cursor_x, text_y, 2.0, font_size), text_color);
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Rebase from source before applying the edit so typing stays on
        // top of any external write that landed since the last paint.
        self.sync_from_source();

        match event {
            WidgetEvent::MouseDown { .. } => {
                // Focus is already set by WidgetTree's click-to-focus
                // (dispatched FocusGained before this handler runs).
                // Just move the cursor to end — precise positioning
                // would need per-glyph metrics.
                self.cursor.set(self.value.borrow().len());
                EventResult::Consumed
            }

            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }

            WidgetEvent::CharInput { ch } if self.focused => {
                if !ch.is_control() {
                    let ch_len = ch.len_utf8();
                    let cursor = self.cursor.get();
                    self.value.borrow_mut().insert(cursor, *ch);
                    self.cursor.set(cursor + ch_len);

                    self.push_to_source();
                    if let Some(handler) = self.on_change.as_mut() {
                        let snapshot = self.value.borrow().clone();
                        handler(&snapshot, ctx);
                    }
                }
                EventResult::Consumed
            }

            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    let cursor = self.cursor.get();
                    if cursor > 0 {
                        let prev = {
                            let v = self.value.borrow();
                            Self::prev_char_boundary(&v, cursor)
                        };
                        self.value.borrow_mut().drain(prev..cursor);
                        self.cursor.set(prev);

                        self.push_to_source();
                        if let Some(handler) = self.on_change.as_mut() {
                            let snapshot = self.value.borrow().clone();
                            handler(&snapshot, ctx);
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Delete) => {
                    let cursor = self.cursor.get();
                    let len = self.value.borrow().len();
                    if cursor < len {
                        let next = {
                            let v = self.value.borrow();
                            Self::next_char_boundary(&v, cursor)
                        };
                        self.value.borrow_mut().drain(cursor..next);

                        self.push_to_source();
                        if let Some(handler) = self.on_change.as_mut() {
                            let snapshot = self.value.borrow().clone();
                            handler(&snapshot, ctx);
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    let cursor = self.cursor.get();
                    if cursor > 0 {
                        let prev = {
                            let v = self.value.borrow();
                            Self::prev_char_boundary(&v, cursor)
                        };
                        self.cursor.set(prev);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let cursor = self.cursor.get();
                    let len = self.value.borrow().len();
                    if cursor < len {
                        let next = {
                            let v = self.value.borrow();
                            Self::next_char_boundary(&v, cursor)
                        };
                        self.cursor.set(next);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Home) => {
                    self.cursor.set(0);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::End) => {
                    self.cursor.set(self.value.borrow().len());
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(handler) = self.on_submit.as_mut() {
                        let snapshot = self.value.borrow().clone();
                        handler(&snapshot, ctx);
                    }
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        }
    }
}
