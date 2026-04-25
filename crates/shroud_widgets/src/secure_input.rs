//! SecureInput widget — password input with zeroization guarantees.
//!
//! Characters are pushed directly into a `SecureString` — no intermediate
//! `String` buffer. Display renders masked characters (●). The `SecureString`
//! is zeroized when the widget is dropped or the owning scope is disposed.
//!
//! ## Clearing (Phase 18d)
//!
//! There is deliberately no `Reactive<SecureString>` binding. Secrets don't
//! belong in the reactive graph: `Reactive::Dynamic` clones on every read
//! and `Signal<String>` would leak plaintext through `get_clone()`. Instead,
//! callers bind a [`ClearTrigger`] via [`SecureInput::clear_on`] and call
//! `trigger.bump()` when the widget should zeroize its buffer. The widget
//! observes the version counter on its next paint/event and clears.

use std::cell::{Cell, RefCell};

use crate::clear_trigger::ClearTrigger;
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
/// - Characters go directly into `SecureString` (no `String` intermediary).
/// - Renders masked characters — the actual text is never rendered.
/// - `SecurityLevel::Protected` — inherits mlock, secure atlas, etc.
/// - `SecureString` zeroizes on drop.
///
/// # Example (conceptual)
/// ```ignore
/// let clear = ClearTrigger::new();
/// let input = SecureInput::new()
///     .placeholder("Enter password")
///     .clear_on(clear)
///     .on_submit(move |pw, _ctx| {
///         unlock(pw);
///         clear.bump(); // zeroize the buffer after submit
///     });
/// ```
pub struct SecureInput {
    /// The secure text content. `RefCell` because [`Widget::paint`] takes
    /// `&self` but may need to clear the buffer when the bound
    /// `ClearTrigger`'s version has changed.
    value: RefCell<SecureString>,
    /// Mask character to display.
    mask_char: char,
    /// Placeholder text (shown when empty).
    placeholder: String,
    /// Font size in pixels (None = theme body size).
    font_size: Option<f32>,
    /// Whether this input has focus.
    focused: bool,
    /// Cursor position (character index). `Cell` so paint-side clears can
    /// reset it to 0.
    cursor: Cell<usize>,
    /// Submit handler, fired on Enter. Receives `&SecureString` so callers
    /// can copy, hash, or consume the value without widening its exposure.
    on_submit: Option<SubmitHandler>,
    /// Optional external clear signal. When `bump()`ed, the widget zeroizes
    /// its buffer on the next paint/event.
    clear_trigger: Option<ClearTrigger>,
    /// Last version observed on `clear_trigger`. A change means we should
    /// clear. Initialized to the trigger's current version at bind time so
    /// binding never spuriously fires a clear.
    last_clear_version: Cell<u32>,
    // Colors (None = read from theme).
    //
    // Mirrors `Input`: the focus ring (Phase 19b) is the canonical
    // signal, so bg/border do not change with focus state.
    bg_color: Option<Color>,
    text_color: Option<Color>,
    placeholder_color: Option<Color>,
    border_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl SecureInput {
    /// Create a new empty secure input.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(SecureString::empty()),
            mask_char: '●',
            placeholder: String::new(),
            font_size: None,
            focused: false,
            cursor: Cell::new(0),
            on_submit: None,
            clear_trigger: None,
            last_clear_version: Cell::new(0),
            bg_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            focus_ring_color: None,
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

    /// Override the keyboard-focus ring color. `None` (the default) reads
    /// `theme.focus.ring_color` each frame.
    pub fn focus_ring_color(mut self, color: Color) -> Self {
        self.focus_ring_color = Some(color);
        self
    }

    /// Register a handler fired when the user presses Enter.
    ///
    /// The handler borrows the `SecureString` — use `expose` inside it to
    /// read the raw bytes, or clone into a caller-owned `SecureString`.
    /// Also receives the current [`EventContext`], so handlers can
    /// transition the UI (`ctx.replace_screen(...)`) in response to an
    /// unlock attempt. The input is *not* cleared after submit; callers
    /// who want that behavior should attach a [`ClearTrigger`] via
    /// [`clear_on`](Self::clear_on) and `bump()` it, or call `clear()` on
    /// a direct `&mut SecureInput` reference.
    pub fn on_submit(mut self, f: impl FnMut(&SecureString, &mut EventContext) + 'static) -> Self {
        self.on_submit = Some(Box::new(f));
        self
    }

    /// Bind a [`ClearTrigger`] that clears (zeroizes) this widget's buffer
    /// whenever the caller invokes `trigger.bump()`.
    ///
    /// The clear is observed on the next paint or event. The initial
    /// version is captured at bind time so merely attaching an
    /// already-bumped trigger doesn't spuriously clear a fresh widget
    /// (whose buffer is empty anyway).
    pub fn clear_on(mut self, trigger: ClearTrigger) -> Self {
        self.last_clear_version.set(trigger.version());
        self.clear_trigger = Some(trigger);
        self
    }

    /// Access the secure value through a closure.
    pub fn expose<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        self.value.borrow().expose(f)
    }

    /// Get the number of characters entered.
    pub fn char_count(&self) -> usize {
        self.value.borrow().char_count()
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.value.borrow().is_empty()
    }

    /// Whether this input currently has focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Clear the input, zeroizing the content.
    pub fn clear(&mut self) {
        self.value.borrow_mut().clear();
        self.cursor.set(0);
    }

    fn resolve_bg(&self, colors: &shroud_core::Colors) -> Color {
        self.bg_color.unwrap_or(colors.input_background)
    }

    fn resolve_border(&self, colors: &shroud_core::Colors) -> Color {
        self.border_color.unwrap_or(colors.input_border)
    }

    /// Observe the clear trigger version and zeroize if it changed. Called
    /// at the top of both paint and event so the clear is visible within
    /// one frame of the `bump()`.
    fn sync_clear(&self) {
        if let Some(trigger) = self.clear_trigger.as_ref() {
            let v = trigger.version();
            if v != self.last_clear_version.get() {
                self.value.borrow_mut().clear();
                self.cursor.set(0);
                self.last_clear_version.set(v);
            }
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

    fn focusable(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new().padding(8.0).min_height(font_size + 20.0)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        self.sync_clear();

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

        let value = self.value.borrow();
        if value.is_empty() {
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
                std::iter::repeat_n(self.mask_char, value.char_count()).collect();

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

            // Cursor (simple line when focused)
            if self.focused {
                let cursor_x = if shaped.glyphs.is_empty() {
                    text_x
                } else {
                    text_x + shaped.width
                };
                ctx.fill_rect(Rect::new(cursor_x, text_y, 2.0, font_size), text_color);
            }
        }

        if self.focused {
            ctx.paint_focus_ring(layout, self.focus_ring_color);
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Observe any queued clear before applying the event. A handler
        // may have bumped the trigger earlier this dispatch cycle.
        self.sync_clear();

        match event {
            WidgetEvent::MouseDown { .. } => {
                // Focus is already set by WidgetTree's click-to-focus
                // path (dispatched FocusGained before this handler runs),
                // so there is nothing widget-specific to do here.
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
                // Push character directly into SecureString — no intermediary
                if !ch.is_control() {
                    self.value.borrow_mut().push(*ch);
                    self.cursor.set(self.cursor.get() + 1);
                }
                EventResult::Consumed
            }

            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    let cursor = self.cursor.get();
                    if cursor > 0 {
                        self.value.borrow_mut().pop();
                        self.cursor.set(cursor - 1);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(handler) = self.on_submit.as_mut() {
                        let value_ref = self.value.borrow();
                        handler(&value_ref, ctx);
                    }
                    // A typical handler bumps the clear trigger after
                    // `add_entry` — re-observe here so the next paint
                    // doesn't render masked characters for the stale value.
                    self.sync_clear();
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        }
    }
}
