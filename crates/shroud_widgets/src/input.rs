//! Input widget — standard text input for non-sensitive data.
//!
//! Unlike `SecureInput`, the value is a plain `String` and rendered
//! as-is (no masking). Supports cursor movement, Home/End, Delete,
//! and `on_change`/`on_submit` callbacks.

use crate::event::{EventContext, EventResult, Key, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;

/// A standard text input field.
///
/// # Example (conceptual)
/// ```ignore
/// let input = Input::new()
///     .placeholder("Enter username")
///     .on_change(|text, _ctx| println!("changed: {text}"))
///     .on_submit(|text, _ctx| println!("submitted: {text}"));
/// ```
type TextCallback = Box<dyn FnMut(&str, &mut EventContext)>;

pub struct Input {
    value: String,
    placeholder: String,
    font_size: Option<f32>,
    focused: bool,
    cursor: usize,
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
            value: String::new(),
            placeholder: String::new(),
            font_size: None,
            focused: false,
            cursor: 0,
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
        self.value = value.into();
        self.cursor = self.value.len();
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
    /// Receives the current value and the [`EventContext`] — ignore the
    /// ctx with `|text, _ctx| { ... }` when tree mutations aren't needed.
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

    /// Get the current value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Whether this input currently has focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Get cursor position (byte offset).
    pub fn cursor(&self) -> usize {
        self.cursor
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

    /// Find the previous char boundary before `pos`.
    fn prev_char_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut i = pos - 1;
        while i > 0 && !self.value.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Find the next char boundary after `pos`.
    fn next_char_boundary(&self, pos: usize) -> usize {
        if pos >= self.value.len() {
            return self.value.len();
        }
        let mut i = pos + 1;
        while i < self.value.len() && !self.value.is_char_boundary(i) {
            i += 1;
        }
        i
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Input {
    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new().padding(8.0).min_height(font_size + 20.0)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
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

        if self.value.is_empty() {
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
            let shaped = ctx.text_engine.shape_text(
                &self.value,
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
                        text_color,
                        glyph.cache_key,
                    );
                }
            }

            // Cursor
            if self.focused {
                // Approximate cursor position: shape text up to cursor byte offset
                let cursor_x = if self.cursor == 0 {
                    text_x
                } else {
                    let before_cursor = &self.value[..self.cursor];
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
        match event {
            WidgetEvent::MouseDown { .. } => {
                self.focused = true;
                // Place cursor at end (precise position would need glyph metrics)
                self.cursor = self.value.len();
                EventResult::Consumed
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }

            WidgetEvent::CharInput { ch } if self.focused => {
                if !ch.is_control() {
                    self.value.insert(self.cursor, *ch);
                    self.cursor += ch.len_utf8();
                    if let Some(handler) = &mut self.on_change {
                        handler(&self.value, ctx);
                    }
                }
                EventResult::Consumed
            }

            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    if self.cursor > 0 {
                        let prev = self.prev_char_boundary(self.cursor);
                        self.value.drain(prev..self.cursor);
                        self.cursor = prev;
                        if let Some(handler) = &mut self.on_change {
                            handler(&self.value, ctx);
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Delete) => {
                    if self.cursor < self.value.len() {
                        let next = self.next_char_boundary(self.cursor);
                        self.value.drain(self.cursor..next);
                        if let Some(handler) = &mut self.on_change {
                            handler(&self.value, ctx);
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    if self.cursor > 0 {
                        self.cursor = self.prev_char_boundary(self.cursor);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowRight) => {
                    if self.cursor < self.value.len() {
                        self.cursor = self.next_char_boundary(self.cursor);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Home) => {
                    self.cursor = 0;
                    EventResult::Consumed
                }
                Key::Named(NamedKey::End) => {
                    self.cursor = self.value.len();
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(handler) = &mut self.on_submit {
                        handler(&self.value, ctx);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Escape) => {
                    self.focused = false;
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        }
    }
}
