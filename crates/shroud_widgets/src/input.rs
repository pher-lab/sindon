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
//!
//! ## Multi-line mode (Phase 25)
//!
//! Calling [`Input::multiline`] flips the widget into a textarea: Enter
//! inserts `\n` (instead of firing `on_submit`), text soft-wraps at the
//! widget's content width, ArrowUp / ArrowDown navigate between hard
//! lines preserving the visual column, and the default height grows to
//! [`Input::lines`] line-heights. All other behavior (signal binding,
//! placeholder, focus ring, on_change) carries over unchanged.
//!
//! ## Numeric mode (Phase 28)
//!
//! Calling [`Input::numeric`] restricts character input to ASCII digits,
//! enables typed bidirectional binding via [`Input::number_value`]
//! (`Signal<i64>`), and applies the [`Input::min_value`] / [`Input::max_value`]
//! clamp every time the buffer parses cleanly. While the field is being
//! edited, intermediate states like an empty buffer or leading zeros are
//! tolerated — the bound signal only updates when the buffer parses. On
//! focus loss the buffer is re-rendered from the signal, so any partial /
//! out-of-range input snaps back to a canonical form (e.g. `"007"` → `"7"`,
//! `""` → the clamped value). External writes to the signal are picked up
//! on the next paint unless the field is currently focused. Numeric mode
//! is mutually exclusive with multi-line mode (the builder asserts in debug).

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

/// Context-only callback (no text payload) — used for [`Input::on_blur`] and
/// [`Input::on_backspace_empty`], where the handler reacts to a focus/key
/// event rather than to the buffer contents.
type CtxCallback = Box<dyn FnMut(&mut EventContext)>;

/// Clamp `v` to the inclusive `[min, max]` range when either bound is set.
/// `None` bounds are unbounded on that side. Used by numeric-mode editing.
fn clamp_opt(v: i64, min: Option<i64>, max: Option<i64>) -> i64 {
    let mut out = v;
    if let Some(lo) = min {
        if out < lo {
            out = lo;
        }
    }
    if let Some(hi) = max {
        if out > hi {
            out = hi;
        }
    }
    out
}

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
    /// Optional external binding for the cursor byte offset. When `Some`,
    /// every paint and event mirrors the caret into the signal (so a sibling
    /// widget like a formatting toolbar can read it), and an external write
    /// is adopted on the next sync (clamped to the buffer, snapped to a char
    /// boundary). Pairs with [`source`](Self::source) for caret-aware inserts.
    cursor_source: Option<Signal<usize>>,
    placeholder: String,
    font_size: Option<f32>,
    focused: bool,
    /// Multi-line / textarea mode. When `true`, Enter inserts `\n`
    /// (instead of firing `on_submit`) and ArrowUp/Down navigate between
    /// hard lines preserving the visual column.
    multiline: bool,
    /// Number of line-heights to size the field to when multi-line is on.
    /// `None` = a small built-in default (3 lines). Ignored in single-line
    /// mode where `min_height` derives from `font_size`.
    line_count: Option<usize>,
    /// "Sticky" column tracked across ArrowUp / ArrowDown so vertical
    /// navigation through ragged lines lands the cursor at the same
    /// visual offset as where the user originally started navigating.
    /// Cleared by any non-vertical edit or motion.
    desired_col: Cell<Option<usize>>,
    /// Numeric (digit-only) input mode. When `true`, [`CharInput`] events
    /// only commit ASCII digits and [`min_value`] / [`max_value`] clamp
    /// the parsed integer on each edit.
    ///
    /// [`CharInput`]: crate::event::WidgetEvent::CharInput
    /// [`min_value`]: Self::min_value
    /// [`max_value`]: Self::max_value
    numeric: bool,
    /// Inclusive lower bound used by numeric mode to clamp the parsed
    /// value. `None` = no lower bound.
    min_value: Option<i64>,
    /// Inclusive upper bound used by numeric mode to clamp the parsed
    /// value. `None` = no upper bound.
    max_value: Option<i64>,
    /// Optional typed binding for numeric mode. When `Some`, the widget
    /// rebases its buffer from the signal on paint (unless focused) and
    /// writes the freshly-parsed-and-clamped value back on every edit.
    number_source: Option<Signal<i64>>,
    on_change: Option<TextCallback>,
    on_submit: Option<TextCallback>,
    /// Fires when the field loses keyboard focus (`FocusLost`), after any
    /// internal canonicalization. Lets apps dismiss transient UI tied to the
    /// field — e.g. an autocomplete suggestion list.
    on_blur: Option<CtxCallback>,
    /// Fires when Backspace is pressed while the buffer is empty. Lets a
    /// chip/tag editor remove the last committed chip when the user keeps
    /// deleting past the start of the text.
    on_backspace_empty: Option<CtxCallback>,
    // Colors (None = read from theme).
    //
    // The focus ring (Phase 19b) is now the canonical signal that this
    // input has keyboard focus, so the bg/border do not change with
    // focus state — the ring is the single source of "I'm focused".
    // Apps that want extra emphasis can supply their own theme overrides.
    bg_color: Option<Color>,
    text_color: Option<Color>,
    placeholder_color: Option<Color>,
    border_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl Input {
    /// Create a new empty input.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(String::new()),
            cursor: Cell::new(0),
            source: None,
            cursor_source: None,
            placeholder: String::new(),
            font_size: None,
            focused: false,
            multiline: false,
            line_count: None,
            desired_col: Cell::new(None),
            numeric: false,
            min_value: None,
            max_value: None,
            number_source: None,
            on_change: None,
            on_submit: None,
            on_blur: None,
            on_backspace_empty: None,
            bg_color: None,
            text_color: None,
            placeholder_color: None,
            border_color: None,
            focus_ring_color: None,
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

    /// Bind this input's cursor (byte offset) to a `Signal<usize>`
    /// (bidirectional).
    ///
    /// On every paint and event the widget mirrors its caret into the signal,
    /// so a sibling widget — e.g. a formatting toolbar — can read where the
    /// caret is *before* it acts. When the signal is written from outside, the
    /// widget adopts that offset on the next sync, clamped to the buffer length
    /// and snapped down to the nearest char boundary.
    ///
    /// Pair with [`value`](Self::value) to insert text at the caret: set the
    /// value signal to the edited text, then set the cursor signal to the new
    /// caret offset. The widget rebases the buffer and adopts the caret on the
    /// next paint, leaving the cursor where the caller asked.
    ///
    /// The signal seeds from the current cursor on bind.
    pub fn cursor_signal(mut self, signal: Signal<usize>) -> Self {
        signal.set(self.cursor.get());
        self.cursor_source = Some(signal);
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

    /// Switch this input to multi-line (textarea) mode.
    ///
    /// In multi-line mode, Enter inserts a newline at the cursor instead of
    /// firing [`on_submit`](Self::on_submit), text soft-wraps at the field's
    /// content width, and ArrowUp / ArrowDown navigate between hard lines
    /// while preserving the visual column. Tab is *not* captured — the focus
    /// manager keeps owning it — so the field stays a friendly form citizen.
    pub fn multiline(mut self) -> Self {
        debug_assert!(
            !self.numeric,
            "Input::multiline() cannot be combined with numeric()"
        );
        self.multiline = true;
        self
    }

    /// Initial visible row count for multi-line mode. Used to size the
    /// field's `min_height` to `lines * line_height + 2 * padding`. Has no
    /// effect when [`multiline`](Self::multiline) is not set. Defaults to
    /// 3 lines.
    pub fn lines(mut self, count: usize) -> Self {
        self.line_count = Some(count.max(1));
        self
    }

    /// Switch this input to numeric (digit-only) mode.
    ///
    /// `CharInput` events are filtered to ASCII digits (`0`..=`9`); other
    /// characters are silently dropped. Combine with
    /// [`min_value`](Self::min_value) / [`max_value`](Self::max_value) to
    /// clamp the parsed integer on every edit, and with
    /// [`number_value`](Self::number_value) for typed bidirectional
    /// binding to a `Signal<i64>`.
    ///
    /// Mutually exclusive with [`multiline`](Self::multiline) — combining
    /// the two trips a `debug_assert` in debug builds.
    pub fn numeric(mut self) -> Self {
        debug_assert!(
            !self.multiline,
            "Input::numeric() cannot be combined with multiline()"
        );
        self.numeric = true;
        self
    }

    /// Inclusive lower bound for numeric mode. Applied every time the
    /// buffer parses as an integer. Has no effect outside numeric mode.
    pub fn min_value(mut self, v: i64) -> Self {
        self.min_value = Some(v);
        self
    }

    /// Inclusive upper bound for numeric mode. Applied every time the
    /// buffer parses as an integer. Has no effect outside numeric mode.
    pub fn max_value(mut self, v: i64) -> Self {
        self.max_value = Some(v);
        self
    }

    /// Bind this input to a `Signal<i64>` (bidirectional, numeric mode).
    ///
    /// Calling this also implicitly enables [`numeric`](Self::numeric).
    /// The widget's buffer seeds from the signal's current value
    /// (rendered without a sign for non-negative integers). On paint, if
    /// the field is *not* focused and the buffer disagrees with the
    /// signal, the buffer is re-rendered from the signal — so external
    /// `signal.set(n)` calls show up in the UI.
    ///
    /// On every edit the buffer is parsed; if it parses cleanly the value
    /// is clamped to `[min_value, max_value]` (when set) and written back
    /// to the signal. An empty or unparseable buffer leaves the signal
    /// untouched, allowing transient editing states. On focus loss the
    /// buffer is re-canonicalized from the signal.
    pub fn number_value(mut self, signal: Signal<i64>) -> Self {
        debug_assert!(
            !self.multiline,
            "Input::number_value() cannot be combined with multiline()"
        );
        // Render the signal's current value verbatim — no construct-time
        // clamp. If the caller seeds with an out-of-range value, the buffer
        // honestly shows it; clamping only kicks in once the user starts
        // editing. Matches the HTML `<input type="number" value={x}>`
        // semantics where `value` is shown as-is regardless of `min/max`.
        let rendered = signal.get().to_string();
        self.cursor.set(rendered.len());
        *self.value.borrow_mut() = rendered;
        self.numeric = true;
        self.number_source = Some(signal);
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

    /// Set a callback for when the field loses keyboard focus.
    ///
    /// Fires on `FocusLost` — after numeric canonicalization (so the bound
    /// signal is already settled) and after `focused` is cleared. The handler
    /// only receives the [`EventContext`]; read the current text from the
    /// bound signal if needed. Typical use is dismissing transient UI the
    /// field owns, like an autocomplete list. Tree mutations should be queued
    /// on the context (e.g. `ctx.rebuild_children(...)`), exactly as in other
    /// handlers.
    pub fn on_blur(mut self, f: impl FnMut(&mut EventContext) + 'static) -> Self {
        self.on_blur = Some(Box::new(f));
        self
    }

    /// Set a callback for Backspace pressed while the buffer is empty.
    ///
    /// A focused input swallows Backspace, so an app can't otherwise observe
    /// it. This hook fires *only* when the field is already empty — a
    /// Backspace at the start of non-empty text remains an inert no-op — which
    /// is the signal a chip/tag editor uses to remove the last committed chip.
    pub fn on_backspace_empty(mut self, f: impl FnMut(&mut EventContext) + 'static) -> Self {
        self.on_backspace_empty = Some(Box::new(f));
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
        self.bg_color.unwrap_or(colors.input_background)
    }

    fn resolve_border(&self, colors: &shroud_core::Colors) -> Color {
        self.border_color.unwrap_or(colors.input_border)
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
    ///
    /// For numeric mode with a [`number_source`](Self::number_source) bound,
    /// the rebase only happens when the field is *not* focused — otherwise
    /// every external write would stomp on the user's mid-typing buffer
    /// (e.g. typing "1" → buffer "1" → signal 1 → re-render to "1", fine;
    /// but typing "" → signal stays → re-render to last value, which would
    /// jump the cursor and prevent deletion). The text-source path keeps its
    /// always-rebase behavior since `Signal<String>` is symmetric.
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
        if let Some(src) = self.number_source.as_ref() {
            if !self.focused {
                // Render raw, no clamp — what `sig.get()` says is what the
                // user sees. `min_value` / `max_value` are documented as
                // applying only to user edits; external out-of-range writes
                // are the caller's responsibility.
                let rendered = src.get().to_string();
                let mut buf = self.value.borrow_mut();
                if *buf != rendered {
                    let new_len = rendered.len();
                    *buf = rendered;
                    if self.cursor.get() > new_len {
                        self.cursor.set(new_len);
                    }
                }
            }
        }
        // Adopt an externally-set caret last, after the buffer has rebased, so
        // the offset is clamped against the *new* text. Snapping down to a char
        // boundary keeps a toolbar that computed a byte offset from panicking
        // the paint-side `&value[..cursor]` slice on a multi-byte codepoint.
        if let Some(csrc) = self.cursor_source.as_ref() {
            let remote = csrc.get();
            if remote != self.cursor.get() {
                let buf = self.value.borrow();
                let mut target = remote.min(buf.len());
                while target > 0 && !buf.is_char_boundary(target) {
                    target -= 1;
                }
                self.cursor.set(target);
            }
        }
    }

    /// Mirror the current caret into the bound cursor signal (if any). Called
    /// at the tail of every event so an external reader sees a fresh offset —
    /// in particular on `FocusLost`, which fires before a clicked toolbar
    /// button's handler runs, so the toolbar reads the pre-blur caret.
    fn push_cursor_to_source(&self) {
        if let Some(csrc) = self.cursor_source.as_ref() {
            if csrc.get() != self.cursor.get() {
                csrc.set(self.cursor.get());
            }
        }
    }

    /// Push the current buffer back to the bound signal (if any). Called
    /// after each edit.
    ///
    /// For numeric mode the buffer is parsed and clamped; an empty or
    /// unparseable buffer leaves the signal untouched so the user can pass
    /// through transient editing states without the signal flapping.
    fn push_to_source(&self) {
        if let Some(src) = self.source.as_ref() {
            src.set(self.value.borrow().clone());
        }
        if let Some(src) = self.number_source.as_ref() {
            let buf = self.value.borrow();
            if let Ok(parsed) = buf.parse::<i64>() {
                let clamped = clamp_opt(parsed, self.min_value, self.max_value);
                if src.get() != clamped {
                    src.set(clamped);
                }
            }
        }
    }

    /// Re-render the buffer from the bound `Signal<i64>` (numeric mode
    /// only). Used on `FocusLost` to snap partial input ("", "007", an
    /// out-of-range number that didn't parse) back to the signal's
    /// canonical decimal form. No clamp here — the signal is already
    /// canonical (it was clamped on the last edit), and rendering raw
    /// keeps "what you see" aligned with "what `sig.get()` returns".
    fn canonicalize_numeric_buffer(&mut self) {
        if let Some(src) = self.number_source.as_ref() {
            let rendered = src.get().to_string();
            let new_len = rendered.len();
            *self.value.borrow_mut() = rendered;
            if self.cursor.get() > new_len {
                self.cursor.set(new_len);
            }
        }
    }

    /// Hard-line index + byte column within that line for the current
    /// cursor. Hard-line model: navigation treats each `\n`-separated
    /// paragraph as one line, regardless of soft wrap. Mirrors what most
    /// simple textareas (and `<textarea>`) do for ArrowUp/Down.
    fn line_col_for_cursor(value: &str, cursor: usize) -> (usize, usize) {
        let prefix = &value[..cursor];
        let line = prefix.matches('\n').count();
        let col = match prefix.rfind('\n') {
            Some(nl) => cursor - (nl + 1),
            None => cursor,
        };
        (line, col)
    }

    /// Inverse of [`line_col_for_cursor`]. Snaps `col` down to the chosen
    /// line's length (so ArrowDown into a shorter line lands at end-of-line)
    /// and to the nearest preceding char boundary (so we never split a
    /// multi-byte codepoint).
    fn cursor_for_line_col(value: &str, line: usize, col: usize) -> usize {
        let mut line_start = 0;
        for _ in 0..line {
            match value[line_start..].find('\n') {
                Some(rel) => line_start += rel + 1,
                None => return value.len(),
            }
        }
        let line_end = value[line_start..]
            .find('\n')
            .map_or(value.len(), |rel| line_start + rel);
        let mut target = (line_start + col).min(line_end);
        while target > line_start && !value.is_char_boundary(target) {
            target -= 1;
        }
        target
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

    fn accepts_text(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let font_size = self.font_size.unwrap_or(16.0);
        if self.multiline {
            let line_height = font_size * 1.2;
            let rows = self.line_count.unwrap_or(3) as f32;
            FlexStyle::new()
                .padding(8.0)
                .min_height(rows * line_height + 16.0)
        } else {
            FlexStyle::new().padding(8.0).min_height(font_size + 20.0)
        }
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        self.sync_from_source();

        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = font_size * 1.2;
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
        // Single-line: vertically center the (one) line of text so the field
        // looks balanced. Multi-line: top-align to padding so the block grows
        // downward as the user types and ArrowUp/Down stay predictable.
        let text_y = if self.multiline {
            layout.origin.y + 8.0
        } else {
            layout.origin.y + (layout.size.height - font_size) / 2.0
        };
        let max_width = layout.size.width - 16.0;
        // Pass max_width as the wrap constraint only in multi-line mode.
        // Single-line inputs intentionally let text overflow horizontally
        // (and don't draw outside their bounds because the wgpu pipeline
        // clips to the widget — adding wrap here would unexpectedly stack
        // text into multiple visual rows inside a one-line input).
        let wrap_width = if self.multiline {
            Some(max_width)
        } else {
            None
        };

        let value = self.value.borrow();
        if value.is_empty() {
            if !self.placeholder.is_empty() {
                let shaped = ctx.text_engine.shape_text(
                    &self.placeholder,
                    font_size,
                    line_height,
                    wrap_width,
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
                let caret = Rect::new(text_x, text_y, 2.0, font_size);
                ctx.fill_rect(caret, text_color);
                // Anchor the IME candidate window at the caret. The OS
                // positions composition UI relative to this rect; without
                // it Win11 IMEs default to a screen-corner location.
                ctx.set_ime_cursor_area(caret);
            }
        } else {
            let shaped = ctx
                .text_engine
                .shape_text(&value, font_size, line_height, wrap_width);

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

            if self.focused {
                let cursor = self.cursor.get();
                let (cx, cy) = if cursor == 0 {
                    (0.0, 0.0)
                } else {
                    ctx.text_engine.cursor_position(
                        &value[..cursor],
                        font_size,
                        line_height,
                        wrap_width,
                    )
                };
                let caret = Rect::new(text_x + cx, text_y + cy, 2.0, font_size);
                ctx.fill_rect(caret, text_color);
                // See the empty-buffer branch above for the rationale —
                // every focused paint re-anchors the IME so it follows
                // the caret as the user types or arrow-keys around.
                ctx.set_ime_cursor_area(caret);
            }
        }

        if self.focused {
            ctx.paint_focus_ring(layout, self.focus_ring_color);
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Rebase from source before applying the edit so typing stays on
        // top of any external write that landed since the last paint.
        self.sync_from_source();

        let result = match event {
            WidgetEvent::MouseDown { .. } => {
                // Focus is already set by WidgetTree's click-to-focus
                // (dispatched FocusGained before this handler runs).
                // Just move the cursor to end — precise positioning
                // would need per-glyph metrics.
                self.cursor.set(self.value.borrow().len());
                self.desired_col.set(None);
                EventResult::Consumed
            }

            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                self.desired_col.set(None);
                if self.numeric {
                    self.canonicalize_numeric_buffer();
                }
                if let Some(handler) = self.on_blur.as_mut() {
                    handler(ctx);
                }
                EventResult::Ignored
            }

            WidgetEvent::CharInput { ch } if self.focused => {
                // Drop control characters, with one exception: a newline in a
                // multi-line field. Pasted text arrives as a burst of CharInput
                // events (see `dispatch_paste` in the event loop), so a textarea
                // must accept `\n` here or a multi-line paste collapses onto a
                // single line. Typed Enter takes the KeyDown path below; this
                // branch only matters for paste. Single-line and numeric fields
                // still drop newlines, flattening a multi-line paste as before.
                let accept = if self.numeric {
                    ch.is_ascii_digit()
                } else if self.multiline && *ch == '\n' {
                    true
                } else {
                    !ch.is_control()
                };
                if accept {
                    let ch_len = ch.len_utf8();
                    let cursor = self.cursor.get();
                    self.value.borrow_mut().insert(cursor, *ch);
                    self.cursor.set(cursor + ch_len);
                    self.desired_col.set(None);

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
                        self.desired_col.set(None);

                        self.push_to_source();
                        if let Some(handler) = self.on_change.as_mut() {
                            let snapshot = self.value.borrow().clone();
                            handler(&snapshot, ctx);
                        }
                    } else if self.value.borrow().is_empty() {
                        // Empty buffer + Backspace: hand off to the app (e.g.
                        // a tag editor removing the last chip). Gated on a
                        // truly-empty buffer so a Backspace at the start of
                        // non-empty text stays an inert no-op.
                        if let Some(handler) = self.on_backspace_empty.as_mut() {
                            handler(ctx);
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
                        self.desired_col.set(None);

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
                    self.desired_col.set(None);
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
                    self.desired_col.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowUp) if self.multiline => {
                    let (line, col) = {
                        let v = self.value.borrow();
                        Self::line_col_for_cursor(&v, self.cursor.get())
                    };
                    if line > 0 {
                        let target_col = self.desired_col.get().unwrap_or(col);
                        let new_cursor = {
                            let v = self.value.borrow();
                            Self::cursor_for_line_col(&v, line - 1, target_col)
                        };
                        self.cursor.set(new_cursor);
                        // Stash the original column so repeated Up moves
                        // through a short line and back into a longer one
                        // restore the original visual column.
                        self.desired_col.set(Some(target_col));
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowDown) if self.multiline => {
                    let (line, col) = {
                        let v = self.value.borrow();
                        Self::line_col_for_cursor(&v, self.cursor.get())
                    };
                    let total_lines = self.value.borrow().matches('\n').count() + 1;
                    if line + 1 < total_lines {
                        let target_col = self.desired_col.get().unwrap_or(col);
                        let new_cursor = {
                            let v = self.value.borrow();
                            Self::cursor_for_line_col(&v, line + 1, target_col)
                        };
                        self.cursor.set(new_cursor);
                        self.desired_col.set(Some(target_col));
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Home) => {
                    self.cursor.set(0);
                    self.desired_col.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::End) => {
                    self.cursor.set(self.value.borrow().len());
                    self.desired_col.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if self.multiline {
                        // Insert a newline at the cursor; on_submit is
                        // intentionally inert in multi-line mode so the
                        // field behaves like a textarea.
                        let cursor = self.cursor.get();
                        self.value.borrow_mut().insert(cursor, '\n');
                        self.cursor.set(cursor + 1);
                        self.desired_col.set(None);

                        self.push_to_source();
                        if let Some(handler) = self.on_change.as_mut() {
                            let snapshot = self.value.borrow().clone();
                            handler(&snapshot, ctx);
                        }
                    } else if let Some(handler) = self.on_submit.as_mut() {
                        let snapshot = self.value.borrow().clone();
                        handler(&snapshot, ctx);
                    }
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        };
        // Mirror the (possibly moved) caret into the bound cursor signal so an
        // external observer — e.g. a formatting toolbar — can read it. No-op
        // when no cursor signal is bound.
        self.push_cursor_to_source();
        result
    }
}
