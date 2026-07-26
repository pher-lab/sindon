//! SecureInput widget — password input with zeroization guarantees.
//!
//! Characters are pushed directly into a `SecureString` — no intermediate
//! `String` buffer. Display renders masked characters (●). The `SecureString`
//! is zeroized when the widget is dropped or the owning scope is disposed.
//!
//! ## Reveal toggle (opt-in)
//!
//! An optional [`revealable`](SecureInput::revealable) eye affordance lets the
//! user show the plaintext. When revealed, the real characters are shaped
//! through the *same* path [`SecureText`](crate::SecureText) uses —
//! [`shape_text_uncached`](shroud_text::TextEngine::shape_text_uncached) into
//! the per-frame-zeroed secure atlas via
//! [`draw_secure_glyph`](crate::PaintContext::draw_secure_glyph) — so showing a
//! secret never lands it in the shape cache or the persistent glyph atlas, and
//! the cosmic-text fork zeroizes every shaped line so nothing lingers on the
//! heap. Reveal is off by default and force-reset to masked on blur / clear.
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
use std::time::Instant;

use crate::caret::{self, CaretBlink};
use crate::clear_trigger::ClearTrigger;
use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{AccessNode, AccessRole, Color, FocusIndicator, Point, Rect, SecurityLevel};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;
use shroud_reactive::animation;
use shroud_security::SecureString;

/// Default maximum length (in bytes) of a `SecureInput` buffer.
///
/// Sized to cover typical passwords, API keys, and master keys without
/// triggering a heap realloc on every keystroke (which would leak the
/// previous buffer's bytes onto a freed page). Override per-widget with
/// [`SecureInput::max_bytes`].
pub const DEFAULT_SECURE_INPUT_MAX_BYTES: usize = 256;

type SubmitHandler = Box<dyn FnMut(&SecureString, &mut EventContext)>;

/// Handler for [`SecureInput::on_length_change`]. Receives the new character
/// count — a length, never the plaintext.
type LengthHandler = Box<dyn FnMut(usize)>;

/// A secure text input field for passwords and sensitive data.
///
/// Key security properties:
/// - Characters go directly into `SecureString` (no `String` intermediary).
/// - Renders masked characters by default; an opt-in
///   [`revealable`](Self::revealable) eye toggle can show the plaintext, and
///   even then it renders through the uncached + secure-atlas path (no shape
///   cache, no persistent atlas, no heap residue).
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
    /// Whether the optional reveal (show-plaintext) eye toggle is offered.
    /// Off unless [`revealable`](Self::revealable) is called. The eye only
    /// paints while the field is non-empty.
    revealable: bool,
    /// Whether the plaintext is currently revealed. Toggled by clicking the
    /// eye; force-reset to `false` on blur and on any clear so a secret is
    /// never left on screen once the field loses focus. `Cell` because the
    /// clear-driven reset in [`sync_clear`](Self::sync_clear) runs from the
    /// `&self` paint path.
    revealed: Cell<bool>,
    /// Placeholder text (shown when empty). Reactive so a signal-driven
    /// relabel (a language switch) reaches it without a tree rebuild. It is
    /// also this field's accessible name — the *only* text a screen reader
    /// gets from a `SecureInput` — so a stale one is announced, not just
    /// drawn. Non-secret by construction: it is a prompt, never the buffer.
    placeholder: Reactive<String>,
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
    /// Fired whenever the character count changes, with the new count (never
    /// the plaintext). `RefCell` so it can fire from `paint` — where a
    /// trigger-driven clear is first observed — as well as from `event`.
    on_length_change: RefCell<Option<LengthHandler>>,
    /// Last character count handed to `on_length_change`, so the callback
    /// fires only on an actual change. Starts at 0 (a fresh buffer is empty).
    last_reported_len: Cell<usize>,
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
    /// Corner radius (px) for the background fill and border stroke. `None`
    /// reads `theme.shape.radius_sm` at paint; a `.radius(px)` override sets
    /// `Some(px)` (`0.0` keeps the sharp rectangle and short-circuits the SDF).
    radius: Option<f32>,
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
    /// When the caret's current solid blink phase began; reset on caret
    /// activity so the caret holds solid while you type. Mirrors `Input`; see
    /// [`caret::blink_phase`](crate::caret::blink_phase).
    blink_ref: Cell<Option<Instant>>,
    /// Caret state at the last focused paint — `(cursor, char count)`. A change
    /// means the user moved the caret or edited, which resets the blink phase.
    /// Cleared on focus loss so the next focus starts solid.
    blink_sig: Cell<Option<(usize, usize)>>,
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
            revealable: false,
            revealed: Cell::new(false),
            placeholder: Reactive::Static(String::new()),
            font_size: None,
            focused: false,
            cursor: Cell::new(0),
            pending_click: Cell::new(None),
            on_submit: None,
            on_length_change: RefCell::new(None),
            last_reported_len: Cell::new(0),
            clear_trigger: None,
            last_clear_version: Cell::new(0),
            bg_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            border_visible: true,
            radius: None,
            pad_x: 8.0,
            pad_y: 8.0,
            min_height_override: None,
            focus_ring_color: None,
            blink_ref: Cell::new(None),
            blink_sig: Cell::new(None),
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
    ///
    /// Callers whose prompt can change while the field is on screen — a
    /// language switch, most commonly — should use
    /// [`reactive_placeholder`](Self::reactive_placeholder) instead.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Reactive::Static(text.into());
        self
    }

    /// Set a placeholder produced by a closure on every frame.
    ///
    /// Mirrors [`Input::reactive_placeholder`](crate::Input::reactive_placeholder).
    /// The closure feeds both the drawn prompt and the accessible name, and is
    /// re-read per frame, so a signal write reaches both on the next one. The
    /// closure must return a *prompt*, never anything derived from the secret
    /// buffer — its return value is placed in the accessibility tree, which
    /// the protected value deliberately never enters.
    pub fn reactive_placeholder(mut self, f: impl Fn() -> String + 'static) -> Self {
        self.placeholder = Reactive::derive(f);
        self
    }

    /// Set the mask character (default: ●).
    pub fn mask(mut self, ch: char) -> Self {
        self.mask_char = ch;
        self
    }

    /// Enable the reveal (show-plaintext) eye toggle.
    ///
    /// Adds a small eye affordance on the trailing edge; clicking it toggles the
    /// field between masked dots and the real characters. The revealed text is
    /// shaped through the *same* uncached + secure-atlas path as
    /// [`SecureText`](crate::SecureText), so showing a secret never lands it in
    /// the shape cache or the persistent glyph atlas — and the cosmic-text fork
    /// zeroizes every shaped line, so nothing lingers on the heap.
    ///
    /// Reveal is off by default and is force-reset to masked whenever the field
    /// loses focus or is cleared, so a secret is never left on screen once the
    /// user moves on. The eye paints only while the field is non-empty.
    ///
    /// The field keeps its no-selection / no-clipboard stance while revealed:
    /// you can *see* the secret but still not select or copy it.
    pub fn revealable(mut self) -> Self {
        self.revealable = true;
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

    /// Round the corners of the background fill and border by `px`. Unset, the
    /// field rounds to `theme.shape.radius_sm`; pass `0.0` for a sharp
    /// rectangle. Symmetric with
    /// [`Input::radius`](crate::Input::radius) /
    /// [`Container::radius`](crate::Container::radius). Negative values clamp to
    /// `0.0`; over-large values are clamped to half the shorter side in the
    /// renderer.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = Some(px.max(0.0));
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

    /// Register a handler fired whenever the character count changes —
    /// on typing, delete/backspace, or a clear (`clear()` or a bumped
    /// [`ClearTrigger`]). The handler receives the *new* count.
    ///
    /// This deliberately mirrors `Input::on_change` but hands out only a
    /// length, never the plaintext: a secret must not flow into a per-keystroke
    /// callback. The count is already observable anyway — the field renders one
    /// mask dot per character — so this exposes nothing the screen doesn't.
    ///
    /// The natural use is driving reactive state: bind a `Signal` and gate a
    /// submit button on emptiness (`s.set(n == 0)`), or show a length / strength
    /// meter. Fires only on a *change*; the initial empty state is not reported,
    /// so initialize any bound signal to match a fresh (empty) field.
    ///
    /// ```ignore
    /// let empty = Signal::new(true);
    /// let field = SecureInput::new().on_length_change(move |n| empty.set(n == 0));
    /// let unlock = Button::new("Unlock").disabled(empty); // dimmed until typed
    /// ```
    pub fn on_length_change(self, f: impl FnMut(usize) + 'static) -> Self {
        *self.on_length_change.borrow_mut() = Some(Box::new(f));
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

    /// Whether the plaintext is currently revealed (see
    /// [`revealable`](Self::revealable)). Always `false` for a field that was
    /// never made revealable.
    pub fn is_revealed(&self) -> bool {
        self.revealed.get()
    }

    /// Clear the input, zeroizing the content.
    pub fn clear(&mut self) {
        self.value.borrow_mut().clear();
        self.cursor.set(0);
        // Nothing left to reveal — drop back to masked so a subsequent entry
        // doesn't start out exposed.
        self.revealed.set(false);
    }

    fn resolve_bg(&self, colors: &shroud_core::Colors) -> Color {
        self.bg_color.unwrap_or(colors.input_background)
    }

    fn resolve_border(&self, colors: &shroud_core::Colors) -> Color {
        self.border_color.unwrap_or(colors.input_border)
    }

    /// Color of the focus indicator (ring in `Ring` mode, focused border in
    /// `Border` mode): the per-widget `focus_ring_color` override if set,
    /// else the theme's `focus.ring_color`. Mirrors `Input`.
    fn focus_indicator_color(&self, focus: &shroud_core::FocusStyle) -> Color {
        self.focus_ring_color.unwrap_or(focus.ring_color)
    }

    /// The trailing-edge square zone that hosts the reveal eye, in the same
    /// coordinate space as `layout`. `None` unless the field is
    /// [`revealable`](Self::revealable).
    ///
    /// Computed from `layout` alone — no theme / font metrics — so `event`
    /// (which has no `PaintContext`) and `paint` agree on the hit region and
    /// the reserved text inset without threading state between them. The zone
    /// is a square the height of the field, right-aligned; the eye is drawn
    /// centered inside it and the text box is inset to clear it. The eye paints
    /// only while non-empty, but reserving the zone whenever `revealable` keeps
    /// the text region from reflowing as content comes and goes.
    fn reveal_hit_rect(&self, layout: Rect) -> Option<Rect> {
        if !self.revealable {
            return None;
        }
        let zone = layout.size.height.min(layout.size.width);
        if zone <= 0.0 {
            return None;
        }
        Some(Rect::new(
            layout.origin.x + layout.size.width - zone,
            layout.origin.y,
            zone,
            layout.size.height,
        ))
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
                self.revealed.set(false);
                self.last_clear_version.set(v);
            }
        }
    }

    /// Fire `on_length_change` if the character count changed since the last
    /// report. Cheap enough (two `usize` reads) to call unconditionally after
    /// any path that might mutate the buffer — the edit keys in `event`, and an
    /// externally observed clear in `paint`. Reads only a length, so the
    /// `value` borrow is released before the handler runs.
    fn emit_len_if_changed(&self) {
        if self.on_length_change.borrow().is_none() {
            return;
        }
        let len = self.value.borrow().char_count();
        if len != self.last_reported_len.get() {
            self.last_reported_len.set(len);
            if let Some(handler) = self.on_length_change.borrow_mut().as_mut() {
                handler(len);
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

    fn accessibility(&self) -> Option<AccessNode> {
        // The whole point of the secret-aware a11y story: expose a *masked*
        // field so a screen reader announces "password" and can land focus
        // here, but NEVER read the characters. `.protected()` force-suppresses
        // the value; the name is the (non-secret) placeholder label, or a
        // generic fallback — the buffer is never touched.
        let placeholder = self.placeholder.get();
        let name = if placeholder.is_empty() {
            "Password".to_string()
        } else {
            placeholder
        };
        Some(
            AccessNode::new(AccessRole::PasswordInput)
                .name(name)
                .protected(),
        )
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
        // A trigger-driven clear is first observed here (not in `event`), so
        // report the resulting length change from paint too.
        self.emit_len_if_changed();

        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let text_color = self.text_color.unwrap_or(ctx.theme.colors.on_surface);
        let placeholder_color = self
            .placeholder_color
            .unwrap_or(ctx.theme.colors.input_placeholder);
        let bg = self.resolve_bg(&ctx.theme.colors);
        // Corner radius: an explicit `.radius(px)` wins, else the theme's small
        // control radius. `radius == 0.0` short-circuits the SDF (sharp rect).
        let radius = self.radius.unwrap_or(ctx.theme.shape.radius_sm);

        // Background fill.
        ctx.fill_rect_rounded(layout, bg, radius);

        // Focus indicator: mirror `Input`. `border_focus` recolors the stroke
        // (theme `Border` mode + a visible border) and suppresses the ring;
        // otherwise the ring paints below (Ring mode, or Border-mode fallback
        // for a borderless field). A text-entry field is always focus-visible
        // when focused (see `Input`) — click focus lights it too, unlike the
        // command widgets that honor the pointer/keyboard `:focus-visible` gate.
        let focus_active = self.focused;
        let border_focus = focus_active
            && self.border_visible
            && ctx.theme.focus.indicator == FocusIndicator::Border;

        // Border: one rounded 1px stroke hugging the inside of the layout edge
        // (the SDF rounds its corners with the same radius), replacing the four
        // sharp edge rects. Skipped entirely when the field is borderless.
        if self.border_visible {
            let border = if border_focus {
                self.focus_indicator_color(&ctx.theme.focus)
            } else {
                self.resolve_border(&ctx.theme.colors)
            };
            ctx.stroke_rect_rounded(layout, border, radius, 1.0);
        }

        let text_x = layout.origin.x + self.pad_x;
        let text_y = layout.origin.y + (layout.size.height - font_size) / 2.0;
        // Reserve the trailing eye zone whenever revealable so the text box
        // doesn't reflow when the eye appears/disappears with (non-)emptiness.
        let reveal_zone = self.reveal_hit_rect(layout).map_or(0.0, |z| z.size.width);
        let max_width = (layout.size.width - 2.0 * self.pad_x - reveal_zone).max(0.0);

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
            let placeholder = self.placeholder.get();
            if !placeholder.is_empty() {
                let shaped = ctx.text_engine.shape_text(
                    &placeholder,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );
                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            text_x + glyph.x,
                            text_y + glyph.y,
                            image,
                            placeholder_color,
                            glyph.cache_key,
                        );
                    }
                }
            }
        } else {
            let char_count = value.char_count();
            let line_height = font_size * 1.2;

            if self.revealed.get() {
                // Reveal: shape and paint the *real* secret through the exact
                // path SecureText uses — `shape_text_uncached` (never the shape
                // cache) into the per-frame-zeroed secure atlas via
                // `draw_secure_glyph`. The cosmic-text fork zeroizes every
                // shaped line, so no plaintext survives on the heap either. The
                // caret is measured against the real glyphs.
                value.expose(|s| {
                    let shaped = ctx.text_engine.shape_text_uncached(
                        s,
                        font_size,
                        line_height,
                        Some(max_width),
                    );

                    for glyph in &shaped.glyphs {
                        if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                            ctx.draw_secure_glyph(
                                text_x + glyph.x,
                                text_y + glyph.y,
                                image,
                                text_color,
                                glyph.cache_key,
                            );
                        }
                    }

                    // Pending click → caret, hit-tested against the real
                    // glyphs. `offset_at_point` shapes through the same uncached
                    // build path, so this too keeps the secret out of the cache.
                    if let Some(click) = self.pending_click.replace(None) {
                        let rel_x = click.x - text_x;
                        let rel_y = (click.y - text_y).max(0.0);
                        let byte = ctx.text_engine.offset_at_point(
                            s,
                            rel_x,
                            rel_y,
                            font_size,
                            line_height,
                            Some(max_width),
                        );
                        let clamped = byte.min(s.len());
                        self.cursor
                            .set(s[..clamped].chars().count().min(char_count));
                    }

                    let cursor = self.cursor.get().min(char_count);
                    if cursor == char_count {
                        if !shaped.glyphs.is_empty() {
                            caret_x = text_x + shaped.width;
                        }
                    } else if cursor > 0 {
                        let byte = s
                            .char_indices()
                            .nth(cursor)
                            .map(|(b, _)| b)
                            .unwrap_or(s.len());
                        let prefix_shaped = ctx.text_engine.shape_text_uncached(
                            &s[..byte],
                            font_size,
                            line_height,
                            Some(max_width),
                        );
                        caret_x = text_x + prefix_shaped.width;
                    }
                    // cursor == 0 → caret_x stays at the left padding (text_x).
                });
            } else {
                // Masked: shape only the homogeneous mask string (not a secret),
                // so it can use the cached path and the standard glyph atlas.
                let mask_str: String = std::iter::repeat_n(self.mask_char, char_count).collect();

                let shaped =
                    ctx.text_engine
                        .shape_text(&mask_str, font_size, line_height, Some(max_width));

                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            text_x + glyph.x,
                            text_y + glyph.y,
                            image,
                            text_color,
                            glyph.cache_key,
                        );
                    }
                }

                // Resolve a pending left-click into a caret position. Hit-test
                // the click against the masked glyphs (`mask_str`) — never the
                // real text — then map the byte offset back to a char index. The
                // mask string is homogeneous, so `byte / mask_char.len_utf8()`
                // is the char index.
                if let Some(click) = self.pending_click.replace(None) {
                    let rel_x = click.x - text_x;
                    let rel_y = (click.y - text_y).max(0.0);
                    let byte = ctx.text_engine.offset_at_point(
                        &mask_str,
                        rel_x,
                        rel_y,
                        font_size,
                        line_height,
                        Some(max_width),
                    );
                    self.cursor
                        .set((byte / self.mask_char.len_utf8()).min(char_count));
                }

                // Caret x at the cursor. The caret can't be measured against the
                // real text (mask glyphs differ in advance from the secret's
                // glyphs), so it's positioned against the rendered dots: shape
                // the masked prefix `[0, cursor)` and take its advance width.
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
                        line_height,
                        Some(max_width),
                    );
                    caret_x = text_x + prefix_shaped.width;
                }
                // cursor == 0 → caret_x stays at the left padding (text_x).
            }

            // Reveal eye affordance — only while there's something to reveal.
            if let Some(zone) = self.reveal_hit_rect(layout) {
                draw_eye(ctx, zone, placeholder_color, self.revealed.get());
            }
        }

        // Caret (simple line when focused) — drawn for both the empty and
        // non-empty cases. With the `:focus-visible` heuristic the ring is
        // suppressed for pointer-driven focus, so without a caret a
        // click-focused empty field gives no visual sign it's active. The
        // caret is the affordance that survives ring suppression (mirrors
        // `Input`, which carets at the line start when empty).
        if self.focused {
            // Blink phase — same model as `Input`, minus IME composition (a
            // focused SecureInput suppresses the IME). The phase resets on any
            // caret activity so the caret holds solid while you type, then we
            // read on/off and vote the next toggle as a timed wake.
            let caret_visible = match caret::caret_blink() {
                CaretBlink::Off => true,
                CaretBlink::Interval(interval) => {
                    let now = animation::now();
                    let sig = (self.cursor.get(), self.value.borrow().char_count());
                    if self.blink_sig.get() != Some(sig) {
                        self.blink_ref.set(Some(now));
                        self.blink_sig.set(Some(sig));
                    }
                    let reference = self.blink_ref.get().unwrap_or(now);
                    let (visible, next) = caret::blink_phase(reference, now, interval);
                    animation::request_frame_at(next);
                    visible
                }
            };

            // Centre the caret on the insertion point, then snap both edges to
            // the device-pixel grid — same fix as `Input`: straddling the
            // boundary keeps the caret from painting over the leading pixel of
            // the dot after it (a right-biased `[x, x+w]` rect does), and
            // rounding the width to whole physical pixels stops the antialiased
            // edges smearing onto the neighbouring dots at fractional DPI.
            // View-only; the cursor index is untouched.
            let (ox, _oy) = ctx.current_offset();
            let caret_w = ctx.snap_device_px(2.0);
            let center_left = (caret_x - caret_w / 2.0).max(text_x);
            let snapped_x = ctx.snap_device_px(center_left + ox) - ox;
            let caret = Rect::new(snapped_x, text_y, caret_w, font_size);
            if caret_visible {
                ctx.fill_rect(caret, text_color);
            }
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
            // Ring shows whenever the field is focused (text entry is always
            // focus-visible, so `focus_active == self.focused` here) and tracks
            // the field's corner radius, so a rounded input gets a rounded ring.
            // Suppressed when `Border` mode already recolored the border above.
            if focus_active && !border_focus {
                ctx.paint_focus_ring(layout, self.focus_ring_color, radius);
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Observe any queued clear before applying the event. A handler
        // may have bumped the trigger earlier this dispatch cycle.
        self.sync_clear();

        let result = match event {
            WidgetEvent::MouseDown { position, button } => {
                // Focus is already set by WidgetTree's click-to-focus path
                // (dispatched FocusGained before this handler runs).
                if *button == MouseButton::Left {
                    // A click on the reveal eye toggles plaintext visibility and
                    // must not also place a caret. The eye is live only while
                    // the field is non-empty (that's when it paints).
                    let on_eye = !self.value.borrow().is_empty()
                        && matches!(
                            self.reveal_hit_rect(layout),
                            Some(hit) if hit.contains(*position)
                        );
                    if on_eye {
                        self.revealed.set(!self.revealed.get());
                    } else {
                        // Record the click so the next paint can place the caret
                        // where the user clicked, hit-tested against the rendered
                        // glyphs (masked dots, or the real text when revealed).
                        self.pending_click.set(Some(*position));
                    }
                }
                EventResult::Consumed
            }

            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                // Never leave a secret revealed once focus leaves the field.
                self.revealed.set(false);
                // Reset the blink phase so the next focus starts solid.
                self.blink_sig.set(None);
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
        };

        // Report any length change from this event (typing, delete, or a
        // clear the Enter handler bumped) after the buffer has settled.
        self.emit_len_if_changed();
        result
    }
}

/// Stamp a round-capped stroke of diameter `thickness` from `a` to `b` by
/// laying overlapping antialiased discs along the segment — the same primitive
/// [`Checkbox`](crate::Checkbox) uses for its checkmark. Keeps line art
/// axis-aligned (rects can't rotate; `push_rotation` is glyph-only) while
/// reading as one smooth stroke with round caps.
fn stroke_round(
    ctx: &mut PaintContext,
    a: (f32, f32),
    b: (f32, f32),
    thickness: f32,
    color: Color,
) {
    let r = thickness / 2.0;
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    // Overlap the discs so the interior never gaps; clamp so a zero-length
    // segment still stamps a single cap.
    let step = (r * 0.5).max(0.35);
    let n = (len / step).ceil().max(1.0) as i32;
    for i in 0..=n {
        let f = i as f32 / n as f32;
        let cx = a.0 + dx * f;
        let cy = a.1 + dy * f;
        ctx.fill_rect_rounded(Rect::new(cx - r, cy - r, thickness, thickness), color, r);
    }
}

/// Draw the reveal "eye" icon centered in `zone`.
///
/// Two mirrored parabolic lids meeting at the corners with a filled pupil disc
/// between them; when `revealed`, a diagonal slash crosses it — the familiar
/// "click to hide" affordance. Built from AA disc stamps (see
/// [`stroke_round`]) so no rotated-rect primitive is needed, consistent with
/// the Checkbox checkmark and the disclosure chevron.
fn draw_eye(ctx: &mut PaintContext, zone: Rect, color: Color, revealed: bool) {
    let cx = zone.origin.x + zone.size.width / 2.0;
    let cy = zone.origin.y + zone.size.height / 2.0;
    // Eye extent relative to the (square) zone — leaves a comfortable margin.
    let s = zone.size.height.min(zone.size.width) * 0.5;
    let half_w = s * 0.5;
    let half_h = s * 0.30;
    let pupil_r = s * 0.15;
    let thickness = (s * 0.09).max(1.3);

    // Two lids: y = ±half_h·(1 − (x/half_w)²), sampled and stamped as arcs that
    // meet at the almond corners (x = ±half_w, y = 0).
    const SAMPLES: usize = 12;
    for sign in [-1.0f32, 1.0] {
        let mut prev: Option<(f32, f32)> = None;
        for i in 0..=SAMPLES {
            let t = i as f32 / SAMPLES as f32;
            let x = -half_w + 2.0 * half_w * t;
            let norm = x / half_w;
            let y = sign * half_h * (1.0 - norm * norm);
            let pt = (cx + x, cy + y);
            if let Some(p) = prev {
                stroke_round(ctx, p, pt, thickness, color);
            }
            prev = Some(pt);
        }
    }

    // Pupil.
    ctx.fill_rect_rounded(
        Rect::new(cx - pupil_r, cy - pupil_r, pupil_r * 2.0, pupil_r * 2.0),
        color,
        pupil_r,
    );

    // Slash across the eye when revealed ("click to hide").
    if revealed {
        let d = s * 0.62;
        stroke_round(ctx, (cx - d, cy + d), (cx + d, cy - d), thickness, color);
    }
}
