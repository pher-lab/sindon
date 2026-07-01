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
use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Point, Rect, SecurityLevel};
use shroud_layout::FlexStyle;
use shroud_security::SecureString;

/// Default maximum length (in bytes) of a `SecureInput` buffer.
///
/// Sized to cover typical passwords, API keys, and master keys without
/// triggering a heap realloc on every keystroke (which would leak the
/// previous buffer's bytes onto a freed page). Override per-widget with
/// [`SecureInput::max_bytes`].
pub const DEFAULT_SECURE_INPUT_MAX_BYTES: usize = 256;

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
    ///
    /// The inner `SecureString` is sized at construction (see
    /// [`max_bytes`](Self::max_bytes)) and never grows. Keystrokes past
    /// the cap are dropped silently.
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
    /// A left-click waiting to be turned into a caret position. Set in
    /// `event` (which has no text engine) and resolved in `paint` against
    /// the *masked* glyphs — the click is hit-tested on the dots, never on
    /// the real text. `None` once consumed.
    pending_click: Cell<Option<Point>>,
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
    /// Whether to draw the 1px border at all. `false` (via [`borderless`]) skips
    /// the stroke, leaving just the (optionally rounded) background fill — handy
    /// for an inline / search-bar look. Mirrors [`Input`]. [`borderless`]:
    /// Self::borderless
    border_visible: bool,
    /// Corner radius (px) for the background fill and border stroke. `0.0` keeps
    /// the historical sharp rectangle and short-circuits the SDF in the shader.
    radius: f32,
    /// Horizontal text inset (px) between the border and the masked text, on
    /// each side. Default 8. Feeds the layout padding, the caret and the click
    /// hit-test. Maps to Tailwind `px-*`. Mirrors [`Input::padding_x`].
    pad_x: f32,
    /// Vertical text inset (px). Default 8. The (single) line is centered, so
    /// this only grows the derived `min_height`. Maps to Tailwind `py-*`.
    /// Mirrors [`Input::padding_y`].
    pad_y: f32,
    /// Explicit `min_height` (px) override for the field's box. `None` derives
    /// it from the font size. Mirrors [`Input::min_height`].
    min_height_override: Option<f32>,
    focus_ring_color: Option<Color>,
}

impl SecureInput {
    /// Create a new empty secure input with the default capacity
    /// (`DEFAULT_SECURE_INPUT_MAX_BYTES`). Use
    /// [`max_bytes`](Self::max_bytes) to override.
    pub fn new() -> Self {
        Self::with_max_bytes(DEFAULT_SECURE_INPUT_MAX_BYTES)
    }

    /// Create a new empty secure input with the given byte capacity.
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            value: RefCell::new(SecureString::with_capacity(max_bytes)),
            mask_char: '●',
            placeholder: String::new(),
            font_size: None,
            focused: false,
            cursor: Cell::new(0),
            pending_click: Cell::new(None),
            on_submit: None,
            clear_trigger: None,
            last_clear_version: Cell::new(0),
            bg_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            border_visible: true,
            radius: 0.0,
            pad_x: 8.0,
            pad_y: 8.0,
            min_height_override: None,
            focus_ring_color: None,
        }
    }

    /// Set the maximum number of bytes this input will accept.
    ///
    /// The underlying `SecureString` is replaced with a fresh buffer of
    /// the given capacity; any previously typed bytes are zeroized. Call
    /// this as a builder step before the widget receives input.
    ///
    /// Keystrokes that would push the buffer past `max_bytes` are
    /// silently dropped (no panic, no audible feedback).
    pub fn max_bytes(mut self, max_bytes: usize) -> Self {
        // Replace the inner SecureString; the old one zeroizes on drop.
        self.value = RefCell::new(SecureString::with_capacity(max_bytes));
        self.cursor.set(0);
        self
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

    /// Override the 1px border color. `None` (the default) reads
    /// `theme.colors.input_border` each frame, so every field tracks the theme;
    /// set this to give one input a distinct frame. Has no effect once
    /// [`borderless`](Self::borderless) is used. Symmetric with
    /// [`Input::border_color`](crate::Input::border_color).
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Drop the border entirely, leaving just the background fill. Useful for
    /// inline editing or a search-bar look where a boxed frame would feel heavy.
    /// Symmetric with [`Input::borderless`](crate::Input::borderless).
    pub fn borderless(mut self) -> Self {
        self.border_visible = false;
        self
    }

    /// Round the corners of the background fill and border by `px`. `0.0` (the
    /// default) keeps the sharp rectangle. Symmetric with
    /// [`Input::radius`](crate::Input::radius) /
    /// [`Container::radius`](crate::Container::radius). Negative values clamp to
    /// `0.0`; over-large values are clamped to half the shorter side in the
    /// renderer.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
        self
    }

    /// Horizontal text inset (px), on each side. Default 8. Maps to Tailwind
    /// `px-*`; moves the caret / hit-test to match. Symmetric with
    /// [`Input::padding_x`](crate::Input::padding_x). Negative values clamp to
    /// `0.0`.
    pub fn padding_x(mut self, px: f32) -> Self {
        self.pad_x = px.max(0.0);
        self
    }

    /// Vertical text inset (px). Default 8. Maps to Tailwind `py-*`. The line is
    /// centered, so this grows the derived [`min_height`](Self::min_height).
    /// Symmetric with [`Input::padding_y`](crate::Input::padding_y). Negative
    /// values clamp to `0.0`.
    pub fn padding_y(mut self, px: f32) -> Self {
        self.pad_y = px.max(0.0);
        self
    }

    /// Explicit minimum box height (px), overriding the font-derived floor. Lets
    /// an app match a design's exact control height (e.g. a `py-3` ≈48px field).
    /// Symmetric with [`Input::min_height`](crate::Input::min_height). Negative
    /// values clamp to `0.0`.
    pub fn min_height(mut self, px: f32) -> Self {
        self.min_height_override = Some(px.max(0.0));
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

    /// Byte offset within the secure buffer of the char at `char_idx`
    /// (clamped to the buffer end when `char_idx == char_count`).
    ///
    /// The widget tracks the caret as a *char* index, but
    /// [`SecureString::insert`]/[`remove`](SecureString::remove) take *byte*
    /// offsets. Computed inside `expose` so the plaintext never escapes the
    /// closure — only the offset (a length) leaves.
    fn byte_offset_of_char(&self, char_idx: usize) -> usize {
        self.value.borrow().expose(|s| {
            s.char_indices()
                .nth(char_idx)
                .map(|(b, _)| b)
                .unwrap_or(s.len())
        })
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

    fn accepts_text(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        let (pad_x, pad_y) = (self.pad_x, self.pad_y);
        // `+ 4.0` preserves the historical `font_size + 20` at the default pad 8.
        let derived = font_size + 2.0 * pad_y + 4.0;
        FlexStyle::new()
            .padding_trbl(pad_y, pad_x, pad_y, pad_x)
            .min_height(self.min_height_override.unwrap_or(derived))
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

        // Background fill. `radius == 0.0` short-circuits the SDF, so this is the
        // historical sharp rect unless the app opted into rounded corners.
        ctx.fill_rect_rounded(layout, bg, self.radius);

        // Border: one rounded 1px stroke hugging the inside of the layout edge
        // (the SDF rounds its corners with the same radius), replacing the four
        // sharp edge rects. Skipped entirely when the field is borderless.
        if self.border_visible {
            let border = self.resolve_border(&ctx.theme.colors);
            ctx.stroke_rect_rounded(layout, border, self.radius, 1.0);
        }

        let text_x = layout.origin.x + self.pad_x;
        let text_y = layout.origin.y + (layout.size.height - font_size) / 2.0;
        let max_width = layout.size.width - 2.0 * self.pad_x;

        let value = self.value.borrow();
        // X of the caret's leading edge. Defaults to the text start, so an
        // empty field carets at the left padding (just like `Input`); the
        // masked branch advances it past the rendered dots.
        let mut caret_x = text_x;
        if value.is_empty() {
            // A click on an empty field has nothing to position — the caret
            // can only sit at the start. Drop any pending click.
            self.pending_click.set(None);
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
            let char_count = value.char_count();
            let mask_str: String = std::iter::repeat_n(self.mask_char, char_count).collect();

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

            // Resolve a pending left-click into a caret position. Hit-test the
            // click against the masked glyphs (`mask_str`) — never the real
            // text — then map the byte offset back to a char index. The mask
            // string is homogeneous, so `byte / mask_char.len_utf8()` is the
            // char index.
            if let Some(click) = self.pending_click.replace(None) {
                let rel_x = click.x - text_x;
                let rel_y = (click.y - text_y).max(0.0);
                let byte = ctx.text_engine.offset_at_point(
                    &mask_str,
                    rel_x,
                    rel_y,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );
                self.cursor
                    .set((byte / self.mask_char.len_utf8()).min(char_count));
            }

            // Caret x at the cursor. The caret can't be measured against the
            // real text (mask glyphs differ in advance from the secret's
            // glyphs), so it's positioned against the rendered dots: shape the
            // masked prefix `[0, cursor)` and take its advance width.
            let cursor = self.cursor.get().min(char_count);
            if cursor == char_count {
                if !shaped.glyphs.is_empty() {
                    caret_x = text_x + shaped.width;
                }
            } else if cursor > 0 {
                let prefix: String = std::iter::repeat_n(self.mask_char, cursor).collect();
                let prefix_shaped = ctx.text_engine.shape_text(
                    &prefix,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );
                caret_x = text_x + prefix_shaped.width;
            }
            // cursor == 0 → caret_x stays at the left padding (text_x).
        }

        // Caret (simple line when focused) — drawn for both the empty and
        // non-empty cases. With the `:focus-visible` heuristic the ring is
        // suppressed for pointer-driven focus, so without a caret a
        // click-focused empty field gives no visual sign it's active. The
        // caret is the affordance that survives ring suppression (mirrors
        // `Input`, which carets at the line start when empty).
        if self.focused {
            let caret = Rect::new(caret_x, text_y, 2.0, font_size);
            ctx.fill_rect(caret, text_color);
        }

        if self.focused {
            // Tier 2 IME bypass: while a SecureInput holds focus, ask
            // the platform window to disconnect the OS IME entirely so
            // keystrokes bypass the composition window an IME engine
            // (or a malicious replacement IME) could observe. No
            // `set_ime_cursor_area` call is needed here — there is no
            // candidate window to anchor. This is a security measure and
            // stays unconditional on focus, independent of the ring below.
            ctx.suppress_ime();
            // Ring follows the `:focus-visible` heuristic and tracks the
            // field's corner radius, so a rounded input gets a rounded ring.
            if ctx.focus_visible() {
                ctx.paint_focus_ring(layout, self.focus_ring_color, self.radius);
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Observe any queued clear before applying the event. A handler
        // may have bumped the trigger earlier this dispatch cycle.
        self.sync_clear();

        match event {
            WidgetEvent::MouseDown { position, button } => {
                // Focus is already set by WidgetTree's click-to-focus path
                // (dispatched FocusGained before this handler runs). Record a
                // left-click so the next paint can place the caret where the
                // user clicked, hit-tested against the masked glyphs.
                if *button == MouseButton::Left {
                    self.pending_click.set(Some(*position));
                }
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
                // Insert the character directly into SecureString at the caret
                // — no intermediary String. Drop the keystroke if it would
                // overflow the fixed capacity (which would panic on insert()).
                // This is the line that enforces the no-realloc invariant: see
                // Phase 20 audit response (H-1). Insertion within capacity does
                // not realloc, so mid-string editing keeps that invariant.
                if !ch.is_control() {
                    let at = self.byte_offset_of_char(self.cursor.get());
                    let mut value = self.value.borrow_mut();
                    if value.remaining_capacity() >= ch.len_utf8() {
                        value.insert(at, *ch);
                        drop(value);
                        self.cursor.set(self.cursor.get() + 1);
                    }
                }
                EventResult::Consumed
            }

            // Caret navigation and edit keys. Note there is deliberately *no*
            // selection model here (no Shift+arrow, Ctrl+A, copy/cut): a
            // selection that could be copied would put the secret on the OS
            // clipboard, defeating the point of SecureInput. The non-secure
            // `Input` carries that model; SecureInput intentionally does not.
            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    let cursor = self.cursor.get();
                    if cursor > 0 {
                        let at = self.byte_offset_of_char(cursor - 1);
                        self.value.borrow_mut().remove(at);
                        self.cursor.set(cursor - 1);
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Delete) => {
                    let cursor = self.cursor.get();
                    let count = self.value.borrow().char_count();
                    if cursor < count {
                        let at = self.byte_offset_of_char(cursor);
                        self.value.borrow_mut().remove(at);
                        // Caret stays put: the char to its right is gone.
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.cursor.set(self.cursor.get().saturating_sub(1));
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let count = self.value.borrow().char_count();
                    self.cursor.set((self.cursor.get() + 1).min(count));
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Home) => {
                    self.cursor.set(0);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::End) => {
                    self.cursor.set(self.value.borrow().char_count());
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
