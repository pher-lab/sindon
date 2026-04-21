//! SecureInput widget — password input with zeroization guarantees.
//!
//! Characters are pushed directly into a `SecureString` — no intermediate
//! `String` buffer. Display renders masked characters (●). The `SecureString`
//! is zeroized when the widget is dropped or the owning scope is disposed.

use crate::event::{EventContext, EventResult, Key, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect, SecurityLevel};
use shroud_layout::FlexStyle;
use shroud_security::SecureString;

type SubmitHandler = Box<dyn FnMut(&SecureString, &mut EventContext)>;

/// A secure text input field for passwords and sensitive data.
///
/// Key security properties:
/// - Characters go directly into `SecureString` (no `String` intermediary)
/// - Renders masked characters — the actual text is never rendered
/// - `SecurityLevel::Protected` — inherits mlock, secure atlas, etc.
/// - `SecureString` zeroizes on drop
///
/// # Example (conceptual)
/// ```ignore
/// let input = SecureInput::new()
///     .placeholder("Enter password")
///     .mask('●');
/// ```
pub struct SecureInput {
    /// The secure text content.
    value: SecureString,
    /// Mask character to display.
    mask_char: char,
    /// Placeholder text (shown when empty).
    placeholder: String,
    /// Font size in pixels (None = theme body size).
    font_size: Option<f32>,
    /// Whether this input has focus.
    focused: bool,
    /// Cursor position (character index).
    cursor: usize,
    /// Submit handler, fired on Enter. Receives `&SecureString` so callers
    /// can copy, hash, or consume the value without widening its exposure.
    on_submit: Option<SubmitHandler>,
    // Colors (None = read from theme)
    bg_color: Option<Color>,
    bg_focused_color: Option<Color>,
    text_color: Option<Color>,
    placeholder_color: Option<Color>,
    border_color: Option<Color>,
    border_focused_color: Option<Color>,
}

impl SecureInput {
    /// Create a new empty secure input.
    pub fn new() -> Self {
        Self {
            value: SecureString::empty(),
            mask_char: '●',
            placeholder: String::new(),
            font_size: None,
            focused: false,
            cursor: 0,
            on_submit: None,
            bg_color: None,
            bg_focused_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            border_focused_color: None,
        }
    }

    /// Set the placeholder text.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Set the mask character (default: ●).
    pub fn mask(mut self, ch: char) -> Self {
        self.mask_char = ch;
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
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

    /// Register a handler fired when the user presses Enter.
    ///
    /// The handler borrows the `SecureString` — use `expose` inside it to
    /// read the raw bytes, or clone into a caller-owned `SecureString`.
    /// Also receives the current [`EventContext`], so handlers can
    /// transition the UI (`ctx.replace_screen(...)`) in response to an
    /// unlock attempt. The input is *not* cleared after submit; callers
    /// who want that behavior should call `clear()` in their handler.
    pub fn on_submit(mut self, f: impl FnMut(&SecureString, &mut EventContext) + 'static) -> Self {
        self.on_submit = Some(Box::new(f));
        self
    }

    /// Access the secure value through a closure.
    pub fn expose<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        self.value.expose(f)
    }

    /// Get the number of characters entered.
    pub fn char_count(&self) -> usize {
        self.value.char_count()
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Whether this input currently has focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Clear the input, zeroizing the content.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
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
}

impl Default for SecureInput {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for SecureInput {
    fn security_level(&self) -> SecurityLevel {
        SecurityLevel::Protected
    }

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

        // Border (1px visual border via inset rect)
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
            // Show placeholder
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
        } else {
            // Render masked characters — never the actual text
            let mask_str: String =
                std::iter::repeat_n(self.mask_char, self.value.char_count()).collect();

            let shaped =
                ctx.text_engine
                    .shape_text(&mask_str, font_size, font_size * 1.2, Some(max_width));

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

            // Cursor (simple blinking line when focused)
            if self.focused {
                let cursor_x = if shaped.glyphs.is_empty() {
                    text_x
                } else {
                    text_x + shaped.width
                };
                ctx.fill_rect(Rect::new(cursor_x, text_y, 2.0, font_size), text_color);
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseDown { .. } => {
                self.focused = true;
                EventResult::Consumed
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }

            WidgetEvent::CharInput { ch } if self.focused => {
                // Push character directly into SecureString — no intermediary
                if !ch.is_control() {
                    self.value.push(*ch);
                    self.cursor += 1;
                }
                EventResult::Consumed
            }

            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    if self.cursor > 0 {
                        self.value.pop();
                        self.cursor -= 1;
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Escape) => {
                    self.focused = false;
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(handler) = &mut self.on_submit {
                        handler(&self.value, ctx);
                    }
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        }
    }
}
