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
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::event::{EventContext, EventResult, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Point, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::Signal;
use zeroize::Zeroizing;

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

/// True for the canonical "command" chord used by select-all / copy / cut:
/// Ctrl-only (Windows/Linux) or Logo/Cmd-only (macOS), with no Shift or Alt.
/// Mirrors the event loop's `is_paste_combo` gating so the four clipboard
/// chords (Ctrl+A/C/X/V) are recognized consistently.
fn is_cmd_combo(mods: Modifiers) -> bool {
    if mods.shift || mods.alt {
        return false;
    }
    let ctrl_only = mods.ctrl && !mods.logo;
    let logo_only = mods.logo && !mods.ctrl;
    ctrl_only || logo_only
}

/// Like [`is_cmd_combo`] but for the Ctrl/Cmd **+ Shift** chord used by the
/// redo binding (`Ctrl+Shift+Z`). [`is_cmd_combo`] deliberately rejects Shift,
/// so the redo path needs its own predicate. The translation layer still
/// promotes Ctrl+Shift+letter to a `KeyDown { Character }` (Ctrl is a non-shift
/// modifier), so this chord reaches `Input::event` like the plain command ones.
fn is_cmd_shift_combo(mods: Modifiers) -> bool {
    if !mods.shift || mods.alt {
        return false;
    }
    let ctrl_only = mods.ctrl && !mods.logo;
    let logo_only = mods.logo && !mods.ctrl;
    ctrl_only || logo_only
}

/// Maximum gap between two primary-button presses for them to count as a
/// double-click (word select). Matches the Windows default double-click time.
const DOUBLE_CLICK_MAX: Duration = Duration::from_millis(500);
/// Maximum pointer travel (px, per axis) between the two presses of a
/// double-click. A press that lands further away starts a fresh selection
/// instead of extending the first into a word select.
const DOUBLE_CLICK_SLOP: f32 = 4.0;

/// Maximum number of states the undo / redo history retains. Bounds both the
/// memory the history holds and — because each [`Snapshot`] keeps a `Zeroizing`
/// copy of the buffer — the number of plaintext copies kept alive. Older states
/// past the cap are evicted (and wiped) from the front.
const UNDO_CAP: usize = 200;

/// Coalescing class for an edit, used to decide whether a new edit folds into
/// the current undo step or starts a fresh one. Consecutive same-kind inserts
/// (or deletes) that continue from where the previous edit left the caret merge
/// into one undo step; a kind change, a caret jump, or a discrete op (selection
/// replace, cut, Enter) starts a new step. This makes a typing burst — and a
/// paste, which arrives as a burst of inserts with no caret move — one undo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
}

/// A point-in-time snapshot of the editable state held in the undo / redo
/// history. `text` is `Zeroizing` so an evicted or cleared entry wipes its
/// plaintext copy rather than leaving it in freed heap — consistent with the
/// framework's zeroize-first stance (`SecureInput` keeps no history at all).
struct Snapshot {
    text: Zeroizing<String>,
    cursor: usize,
    anchor: Option<usize>,
}

/// Character class used by double-click word expansion. A double-click selects
/// the maximal run of same-class characters under the caret, so a click in a
/// word grabs the word, a click in whitespace grabs the gap, and a click in a
/// punctuation run grabs that run.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    /// Alphanumeric or `_` — the "word" characters.
    Word,
    /// Whitespace (including `\n`, so word runs never cross a hard line break).
    Space,
    /// Everything else (punctuation, symbols).
    Other,
}

fn classify(c: char) -> CharClass {
    if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Other
    }
}

/// Expand a caret `offset` to the byte range of the "word" under it, used by
/// double-click selection. The reference class is the character to the right of
/// the caret, except when that is whitespace (or we're at end-of-text): then we
/// look left, so clicking at a word's trailing edge still grabs the word rather
/// than the following gap. `offset` is clamped and treated as a char boundary;
/// expansion always stops on char boundaries, so multibyte text is safe.
fn word_bounds(s: &str, offset: usize) -> (usize, usize) {
    if s.is_empty() {
        return (0, 0);
    }
    let offset = offset.min(s.len());
    let right = s[offset..].chars().next();
    let left = s[..offset].chars().next_back();
    // Prefer a non-space reference so a click between a word and a trailing
    // space selects the word, not the space.
    let use_left = match (left, right) {
        (Some(l), Some(r)) => classify(r) == CharClass::Space && classify(l) != CharClass::Space,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => return (0, 0),
    };
    let ref_class = if use_left {
        classify(left.unwrap())
    } else {
        classify(right.unwrap())
    };
    let mut lo = offset;
    while lo > 0 {
        let prev = s[..lo].chars().next_back().unwrap();
        if classify(prev) == ref_class {
            lo -= prev.len_utf8();
        } else {
            break;
        }
    }
    let mut hi = offset;
    while hi < s.len() {
        let next = s[hi..].chars().next().unwrap();
        if classify(next) == ref_class {
            hi += next.len_utf8();
        } else {
            break;
        }
    }
    (lo, hi)
}

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
    /// Selection anchor (byte offset). When `Some`, the selection spans
    /// `[min(anchor, cursor), max(anchor, cursor))` and the caret (active end)
    /// is always `cursor`. `None` means no selection — just a caret. `Cell`
    /// so paint can collapse / clamp it alongside the cursor.
    selection_anchor: Cell<Option<usize>>,
    /// A click / drag position waiting to be resolved against the shaped text
    /// at paint time. `Input::event` has no text engine, so a precise
    /// click-to-caret hit-test is deferred to `paint` (which holds the
    /// engine) — the same paint-time hit-cache idiom `TextWidget` uses for
    /// link clicks. The bool is `extend`: `true` moves only the active end
    /// (drag / shift-click), `false` plants a fresh collapsed caret.
    pending_hit: Cell<Option<(Point, bool)>>,
    /// Whether a primary-button drag is in progress — set on MouseDown,
    /// cleared on MouseUp / FocusLost. While set, MouseMove extends the
    /// selection's active end.
    selecting: Cell<bool>,
    /// Timestamp + position of the last primary press, used to recognize a
    /// double-click. A second press within [`DOUBLE_CLICK_MAX`] and
    /// [`DOUBLE_CLICK_SLOP`] of the first promotes the pending hit to a word
    /// select. Reset after a double fires so a triple-click doesn't chain.
    last_click: Cell<Option<(Instant, Point)>>,
    /// Set on a recognized double-click; consumed at paint time to expand the
    /// resolved caret offset to the surrounding word via [`word_bounds`].
    pending_word_select: Cell<bool>,
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
    /// Optional external binding for the selection range, mirrored to a sibling
    /// (e.g. a formatting toolbar that wraps the selection). Bidirectional,
    /// like [`cursor_source`](Self::cursor_source): the widget writes its
    /// current sorted `Some((lo, hi))` — or `None` when nothing is selected —
    /// on every event, and adopts an external write on the next sync (clamped
    /// to the buffer, snapped to char boundaries; the caret follows `hi`). A
    /// caller that rewrites the value should also write the intended post-edit
    /// selection here — `Some(range)` to re-select, `None` to clear.
    selection_source: Option<Signal<Option<(usize, usize)>>>,
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
    /// Override for the selection highlight color. `None` reads
    /// `theme.colors.selection_background` each frame.
    selection_color: Option<Color>,
    /// Undo history: states to return to, oldest at the front. Bounded to
    /// [`UNDO_CAP`]; the front is evicted (and wiped) past the cap. `RefCell`
    /// because a checkpoint can be recorded from the interior-mutable `&self`
    /// paths (and the history is cleared from `sync_from_source`, also `&self`).
    undo_stack: RefCell<VecDeque<Snapshot>>,
    /// Redo history: states undone away from, cleared on any fresh edit.
    redo_stack: RefCell<VecDeque<Snapshot>>,
    /// Coalescing tag for the in-progress undo step: the kind of the last edit
    /// and the caret offset it left behind. A new edit of the same kind whose
    /// pre-edit caret matches the stored offset folds into the current step
    /// instead of pushing a checkpoint. `None` forces the next edit to
    /// checkpoint — set after a discrete op, a caret move, undo/redo, or an
    /// external value change.
    last_edit: Cell<Option<(EditKind, usize)>>,
}

impl Input {
    /// Create a new empty input.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(String::new()),
            cursor: Cell::new(0),
            selection_anchor: Cell::new(None),
            pending_hit: Cell::new(None),
            selecting: Cell::new(false),
            last_click: Cell::new(None),
            pending_word_select: Cell::new(false),
            source: None,
            cursor_source: None,
            selection_source: None,
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
            selection_color: None,
            undo_stack: RefCell::new(VecDeque::new()),
            redo_stack: RefCell::new(VecDeque::new()),
            last_edit: Cell::new(None),
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

    /// Mirror this input's selection range into a `Signal<Option<(usize, usize)>>`
    /// so a sibling widget can read it.
    ///
    /// The signal carries the sorted byte range `Some((lo, hi))` while text is
    /// selected and `None` when it isn't, refreshed on every event — including
    /// `FocusLost`, which fires *before* a clicked toolbar button's handler, so
    /// the toolbar reads the pre-blur selection.
    ///
    /// Bidirectional: an external write is adopted on the next sync (clamped to
    /// the buffer and snapped to char boundaries, with the caret following the
    /// range's upper bound). To act on the selection (e.g. wrap it in `**`),
    /// read the range here, rewrite the text via [`value`](Self::value), then
    /// write the intended *post-edit* range back — `Some(range)` to re-select
    /// the new text, `None` to clear. Without an explicit write the selection
    /// collapses when the value changes (the stale range can't outlive the
    /// rewrite); the write is what lets a toolbar re-select what it just
    /// wrapped. Pairs with [`cursor_signal`](Self::cursor_signal) for the caret.
    ///
    /// The signal seeds from the current selection on bind.
    pub fn selection_signal(mut self, signal: Signal<Option<(usize, usize)>>) -> Self {
        signal.set(self.selection_range());
        self.selection_source = Some(signal);
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

    /// Override the text-selection highlight color. `None` (the default)
    /// reads `theme.colors.selection_background` each frame. A translucent
    /// color keeps the selected glyphs legible on top.
    pub fn selection_color(mut self, color: Color) -> Self {
        self.selection_color = Some(color);
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

    /// Whether any text is currently selected.
    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some()
    }

    /// The selected substring, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (lo, hi) = self.selection_range()?;
        Some(self.value.borrow()[lo..hi].to_string())
    }

    /// The current selection as a sorted `(lo, hi)` byte range, or `None`
    /// when there is no selection (no anchor, or a collapsed one where
    /// `anchor == cursor`).
    fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor.get()?;
        let cursor = self.cursor.get();
        let lo = anchor.min(cursor);
        let hi = anchor.max(cursor);
        if lo < hi { Some((lo, hi)) } else { None }
    }

    /// Drop the selection, leaving the caret where it is.
    fn clear_selection(&self) {
        self.selection_anchor.set(None);
    }

    /// Begin (or keep) a selection anchored at the current caret. The first
    /// Shift+motion pins the anchor; later ones just move the active end.
    fn ensure_anchor(&self) {
        if self.selection_anchor.get().is_none() {
            self.selection_anchor.set(Some(self.cursor.get()));
        }
    }

    /// Select the whole buffer (Ctrl/Cmd+A): anchor at the start, caret at
    /// the end.
    fn select_all(&self) {
        let len = self.value.borrow().len();
        self.selection_anchor.set(Some(0));
        self.cursor.set(len);
        self.desired_col.set(None);
        self.last_edit.set(None);
    }

    /// Delete the current selection if any: remove `[lo, hi)`, move the caret
    /// to `lo`, clear the anchor. Returns whether anything was removed — the
    /// caller composes the follow-up (`push_to_source` / `on_change`); this
    /// only mutates the buffer + caret so it can be shared by typing,
    /// Backspace, Delete, and Cut.
    fn delete_selection(&mut self) -> bool {
        if let Some((lo, hi)) = self.selection_range() {
            self.value.borrow_mut().drain(lo..hi);
            self.cursor.set(lo);
            self.selection_anchor.set(None);
            self.desired_col.set(None);
            true
        } else {
            false
        }
    }

    /// Capture the current editable state for the history.
    fn current_snapshot(&self) -> Snapshot {
        Snapshot {
            text: Zeroizing::new(self.value.borrow().clone()),
            cursor: self.cursor.get(),
            anchor: self.selection_anchor.get(),
        }
    }

    /// Restore an editable state pulled from the history. The caret / anchor
    /// came from the same snapshot as the text, so they're already in range —
    /// no clamping needed. `desired_col` is dropped (vertical-nav state is not
    /// part of the undo model).
    fn apply_snapshot(&self, snap: &Snapshot) {
        self.value.borrow_mut().clear();
        self.value.borrow_mut().push_str(&snap.text);
        self.cursor.set(snap.cursor);
        self.selection_anchor.set(snap.anchor);
        self.desired_col.set(None);
    }

    /// Record an undo checkpoint *before* an edit mutates the buffer — unless
    /// the edit coalesces into the current step. `coalescable` is `false` for
    /// discrete operations (selection replace, cut, Enter) so they always start
    /// a fresh step. Clears the redo stack: a new edit forks history.
    fn begin_edit(&self, kind: EditKind, coalescable: bool) {
        let coalesce = coalescable
            && matches!(self.last_edit.get(), Some((k, pos)) if k == kind && pos == self.cursor.get());
        if coalesce {
            return;
        }
        let snap = self.current_snapshot();
        let mut undo = self.undo_stack.borrow_mut();
        undo.push_back(snap);
        while undo.len() > UNDO_CAP {
            undo.pop_front();
        }
        drop(undo);
        self.redo_stack.borrow_mut().clear();
    }

    /// Finish an edit: update the coalescing tag, then push the fresh buffer to
    /// the bound signal and fire `on_change` — the same tail every edit path
    /// shares. `break_run` forces the next edit to start a fresh undo step
    /// (used by discrete ops so they don't absorb the following keystroke).
    fn commit_edit(&mut self, kind: EditKind, break_run: bool, ctx: &mut EventContext) {
        self.last_edit.set(if break_run {
            None
        } else {
            Some((kind, self.cursor.get()))
        });
        self.push_to_source();
        if let Some(handler) = self.on_change.as_mut() {
            let snapshot = self.value.borrow().clone();
            handler(&snapshot, ctx);
        }
    }

    /// Undo the most recent step: stash the current state on the redo stack and
    /// restore the previous one, then propagate like a normal edit (push to the
    /// bound signal + fire `on_change`). Returns whether anything was undone.
    fn undo(&mut self, ctx: &mut EventContext) -> bool {
        let Some(prev) = self.undo_stack.borrow_mut().pop_back() else {
            return false;
        };
        self.redo_stack
            .borrow_mut()
            .push_back(self.current_snapshot());
        self.apply_snapshot(&prev);
        self.last_edit.set(None);
        self.propagate_history_change(ctx);
        true
    }

    /// Redo the most recently undone step. Mirror image of [`undo`](Self::undo).
    fn redo(&mut self, ctx: &mut EventContext) -> bool {
        let Some(next) = self.redo_stack.borrow_mut().pop_back() else {
            return false;
        };
        self.undo_stack
            .borrow_mut()
            .push_back(self.current_snapshot());
        self.apply_snapshot(&next);
        self.last_edit.set(None);
        self.propagate_history_change(ctx);
        true
    }

    /// Shared tail for undo / redo: mirror the restored buffer to the bound
    /// signal and notify `on_change`, exactly as a keystroke would, so the
    /// bound state / preview / observers see the change. Because this writes the
    /// source, the next `sync_from_source` sees `source == buffer` and is a
    /// no-op, so the restore is not immediately clobbered.
    fn propagate_history_change(&mut self, ctx: &mut EventContext) {
        self.push_to_source();
        if let Some(handler) = self.on_change.as_mut() {
            let snapshot = self.value.borrow().clone();
            handler(&snapshot, ctx);
        }
    }

    /// Drop the entire undo / redo history. Called when the buffer is rebased
    /// from an external write (note switch, toolbar wrap, programmatic set):
    /// undo must never cross such a boundary, so history starts fresh after it.
    fn clear_history(&self) {
        self.undo_stack.borrow_mut().clear();
        self.redo_stack.borrow_mut().clear();
        self.last_edit.set(None);
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
                // An external rewrite (e.g. a toolbar wrapping the selection)
                // invalidates the byte offsets the selection was anchored on —
                // drop it so a stale highlight can't outlive the edit.
                self.selection_anchor.set(None);
                // The undo history belongs to the text we just replaced; a note
                // switch or programmatic set must not be undoable into a
                // *different* document, so start history fresh after the rebase.
                self.clear_history();
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
                    self.clear_history();
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
        // Adopt an externally-set selection range last of all — after the
        // buffer rebased and the value-change drop above ran — so a caller can
        // set a fresh selection that survives the edit. This is what lets the
        // toolbar re-select the inner text it just wrapped: it writes the new
        // range into the bound signal alongside the new value, and the widget
        // adopts it here (clamped + char-boundary-snapped) rather than leaving
        // the collapsed caret the rebase produced. A `None` clears the
        // selection; the active end (caret) follows the range's upper bound.
        if let Some(ssrc) = self.selection_source.as_ref() {
            let remote = ssrc.get();
            if remote != self.selection_range() {
                match remote {
                    Some((lo, hi)) => {
                        let buf = self.value.borrow();
                        let snap = |mut i: usize| {
                            i = i.min(buf.len());
                            while i > 0 && !buf.is_char_boundary(i) {
                                i -= 1;
                            }
                            i
                        };
                        let lo = snap(lo);
                        let hi = snap(hi).max(lo);
                        drop(buf);
                        if lo < hi {
                            self.selection_anchor.set(Some(lo));
                            self.cursor.set(hi);
                        } else {
                            self.selection_anchor.set(None);
                        }
                    }
                    None => self.selection_anchor.set(None),
                }
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

    /// Mirror the current selection range into the bound selection signal (if
    /// any). Called alongside [`push_cursor_to_source`] so a sibling reader sees
    /// a fresh range — in particular on `FocusLost`, which fires before a
    /// clicked toolbar button's handler runs.
    fn push_selection_to_source(&self) {
        if let Some(ssrc) = self.selection_source.as_ref() {
            let current = self.selection_range();
            if ssrc.get() != current {
                ssrc.set(current);
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

        // Resolve a deferred click / drag against the shaped text now that the
        // engine + geometry are in hand (the event handler has neither — see
        // `pending_hit`). `extend` keeps the anchor and moves only the active
        // end; otherwise the caret collapses to a fresh, selection-free point.
        if let Some((pos, extend)) = self.pending_hit.take() {
            let rel_x = pos.x - text_x;
            let rel_y = pos.y - text_y;
            let offset = {
                let v = self.value.borrow();
                ctx.text_engine.offset_at_point(
                    &v,
                    rel_x,
                    rel_y,
                    font_size,
                    line_height,
                    wrap_width,
                )
            };
            if self.pending_word_select.take() {
                // Double-click: select the whole word under the resolved caret.
                let (lo, hi) = {
                    let v = self.value.borrow();
                    word_bounds(&v, offset)
                };
                self.selection_anchor.set(Some(lo));
                self.cursor.set(hi);
            } else {
                if extend {
                    self.ensure_anchor();
                } else {
                    self.selection_anchor.set(None);
                }
                self.cursor.set(offset);
            }
            self.desired_col.set(None);
            // A click / drag moves the caret outside the edit path, so end any
            // in-progress coalescing run — the next keystroke starts a fresh
            // undo step rather than merging with text typed before the click.
            self.last_edit.set(None);
            // Mirror the moved caret + selection so bound signals (and the next
            // event's `sync_from_source`) agree with the paint-resolved hit.
            self.push_cursor_to_source();
            self.push_selection_to_source();
        }

        // Selection highlight, painted behind the glyphs so the (opaque) text
        // stays legible on top of the translucent fill.
        if self.focused {
            if let Some((lo, hi)) = self.selection_range() {
                let sel_color = self
                    .selection_color
                    .unwrap_or(ctx.theme.colors.selection_background);
                let rects = {
                    let v = self.value.borrow();
                    ctx.text_engine
                        .selection_rects(&v, lo, hi, font_size, line_height, wrap_width)
                };
                for r in rects {
                    ctx.fill_rect(
                        Rect::new(
                            text_x + r.origin.x,
                            text_y + r.origin.y,
                            r.size.width,
                            r.size.height,
                        ),
                        sel_color,
                    );
                }
            }
        }

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
            WidgetEvent::MouseDown { position, button } => {
                // Focus is already set by WidgetTree's click-to-focus
                // (dispatched FocusGained before this handler runs). Defer
                // the precise caret hit-test to paint (no text engine here):
                // plant a pending click and begin a potential drag. `extend`
                // follows Shift, so Shift+click stretches the selection from
                // the current caret to the click point.
                if *button == MouseButton::Left {
                    // Recognize a double-click (two quick presses at the same
                    // spot) and promote it to a word select, resolved at paint.
                    let now = Instant::now();
                    let is_double = self.last_click.get().is_some_and(|(t, p)| {
                        now.duration_since(t) <= DOUBLE_CLICK_MAX
                            && (p.x - position.x).abs() <= DOUBLE_CLICK_SLOP
                            && (p.y - position.y).abs() <= DOUBLE_CLICK_SLOP
                    });
                    // Reset the chain after a double so a third press starts
                    // fresh rather than registering as another double.
                    self.last_click.set(if is_double {
                        None
                    } else {
                        Some((now, *position))
                    });
                    self.pending_hit.set(Some((*position, ctx.modifiers.shift)));
                    self.pending_word_select.set(is_double);
                    self.selecting.set(true);
                    self.desired_col.set(None);
                    // Capture the pointer so the drag keeps being delivered
                    // (and is reliably ended) even when the cursor leaves the
                    // field's rect — the tree routes MouseMove/Up straight here.
                    ctx.capture_pointer();
                }
                EventResult::Consumed
            }

            WidgetEvent::MouseMove { position } if self.selecting.get() => {
                // Drag with the button held: extend the selection's active
                // end to the new point (resolved at paint).
                self.pending_hit.set(Some((*position, true)));
                EventResult::Consumed
            }

            WidgetEvent::MouseUp { .. } => {
                // End of a drag (or a plain click): drop the capture and stop
                // extending. Consume only when we were actually dragging, so a
                // release that wasn't ours doesn't shadow another handler.
                let was_selecting = self.selecting.replace(false);
                if was_selecting {
                    ctx.release_pointer();
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }

            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }

            WidgetEvent::FocusLost => {
                self.focused = false;
                // Releasing focus mid-drag (e.g. programmatic refocus) ends the
                // drag; drop the capture too so it can't outlive the selection.
                if self.selecting.replace(false) {
                    ctx.release_pointer();
                }
                self.pending_word_select.set(false);
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
                    // Typing over a selection replaces it (the caret lands at
                    // `lo` before the insert). No-op when nothing is selected.
                    // A selection replace is a discrete undo step; plain typing
                    // coalesces into the current run (so does a paste burst,
                    // which arrives here as inserts with no caret move between).
                    let had_selection = self.selection_range().is_some();
                    self.begin_edit(EditKind::Insert, !had_selection);
                    self.delete_selection();
                    let ch_len = ch.len_utf8();
                    let cursor = self.cursor.get();
                    self.value.borrow_mut().insert(cursor, *ch);
                    self.cursor.set(cursor + ch_len);
                    self.desired_col.set(None);
                    // `had_selection` only decided whether this insert *starts*
                    // a fresh step (above); typing that follows still coalesces
                    // into it, so the run isn't broken here.
                    self.commit_edit(EditKind::Insert, false, ctx);
                }
                EventResult::Consumed
            }

            WidgetEvent::KeyDown { key } if self.focused => match key {
                Key::Named(NamedKey::Backspace) => {
                    // A selection is deleted as a unit (a discrete undo step);
                    // otherwise fall back to the single-char delete (which
                    // coalesces with a run of Backspaces) and the empty-buffer
                    // hand-off.
                    if self.selection_range().is_some() {
                        self.begin_edit(EditKind::Delete, false);
                        self.delete_selection();
                        self.commit_edit(EditKind::Delete, true, ctx);
                    } else {
                        let cursor = self.cursor.get();
                        if cursor > 0 {
                            let prev = {
                                let v = self.value.borrow();
                                Self::prev_char_boundary(&v, cursor)
                            };
                            self.begin_edit(EditKind::Delete, true);
                            self.value.borrow_mut().drain(prev..cursor);
                            self.cursor.set(prev);
                            self.desired_col.set(None);
                            self.commit_edit(EditKind::Delete, false, ctx);
                        } else if self.value.borrow().is_empty() {
                            // Empty buffer + Backspace: hand off to the app
                            // (e.g. a tag editor removing the last chip). Gated
                            // on a truly-empty buffer so a Backspace at the
                            // start of non-empty text stays an inert no-op.
                            if let Some(handler) = self.on_backspace_empty.as_mut() {
                                handler(ctx);
                            }
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Delete) => {
                    // A selection is deleted as a unit (a discrete undo step);
                    // otherwise delete the single char to the right of the
                    // caret (coalescing with a run of forward Deletes).
                    if self.selection_range().is_some() {
                        self.begin_edit(EditKind::Delete, false);
                        self.delete_selection();
                        self.commit_edit(EditKind::Delete, true, ctx);
                    } else {
                        let cursor = self.cursor.get();
                        let len = self.value.borrow().len();
                        if cursor < len {
                            let next = {
                                let v = self.value.borrow();
                                Self::next_char_boundary(&v, cursor)
                            };
                            self.begin_edit(EditKind::Delete, true);
                            self.value.borrow_mut().drain(cursor..next);
                            self.desired_col.set(None);
                            self.commit_edit(EditKind::Delete, false, ctx);
                        }
                    }
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    if ctx.modifiers.shift {
                        // Extend selection one char left.
                        self.ensure_anchor();
                        let cursor = self.cursor.get();
                        if cursor > 0 {
                            let prev = {
                                let v = self.value.borrow();
                                Self::prev_char_boundary(&v, cursor)
                            };
                            self.cursor.set(prev);
                        }
                    } else if let Some((lo, _hi)) = self.selection_range() {
                        // Plain ArrowLeft with a selection collapses to its
                        // left edge (no per-char move).
                        self.cursor.set(lo);
                        self.clear_selection();
                    } else {
                        let cursor = self.cursor.get();
                        if cursor > 0 {
                            let prev = {
                                let v = self.value.borrow();
                                Self::prev_char_boundary(&v, cursor)
                            };
                            self.cursor.set(prev);
                        }
                    }
                    self.desired_col.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowRight) => {
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                        let cursor = self.cursor.get();
                        let len = self.value.borrow().len();
                        if cursor < len {
                            let next = {
                                let v = self.value.borrow();
                                Self::next_char_boundary(&v, cursor)
                            };
                            self.cursor.set(next);
                        }
                    } else if let Some((_lo, hi)) = self.selection_range() {
                        // Plain ArrowRight with a selection collapses to its
                        // right edge.
                        self.cursor.set(hi);
                        self.clear_selection();
                    } else {
                        let cursor = self.cursor.get();
                        let len = self.value.borrow().len();
                        if cursor < len {
                            let next = {
                                let v = self.value.borrow();
                                Self::next_char_boundary(&v, cursor)
                            };
                            self.cursor.set(next);
                        }
                    }
                    self.desired_col.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowUp) if self.multiline => {
                    // Shift extends the selection; a plain vertical move drops
                    // it.
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
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
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowDown) if self.multiline => {
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
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
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Home) => {
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
                    self.cursor.set(0);
                    self.desired_col.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::End) => {
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
                    self.cursor.set(self.value.borrow().len());
                    self.desired_col.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if self.multiline {
                        // Insert a newline at the cursor; on_submit is
                        // intentionally inert in multi-line mode so the field
                        // behaves like a textarea. A newline is its own discrete
                        // undo step — it doesn't merge with the typing on either
                        // side.
                        let cursor = self.cursor.get();
                        self.begin_edit(EditKind::Insert, false);
                        self.value.borrow_mut().insert(cursor, '\n');
                        self.cursor.set(cursor + 1);
                        self.desired_col.set(None);
                        self.commit_edit(EditKind::Insert, true, ctx);
                    } else if let Some(handler) = self.on_submit.as_mut() {
                        let snapshot = self.value.borrow().clone();
                        handler(&snapshot, ctx);
                    }
                    EventResult::Consumed
                }
                // Ctrl/Cmd+Shift+Z = redo (the mac / browser convention). This
                // chord carries Shift, so it can't match the `is_cmd_combo` arm
                // below — it needs its own guard ahead of it. Matched
                // case-insensitively since the promoted char may arrive as
                // 'z' or 'Z' depending on the platform's shift handling.
                Key::Character(c)
                    if is_cmd_shift_combo(ctx.modifiers) && c.eq_ignore_ascii_case(&'z') =>
                {
                    self.redo(ctx);
                    EventResult::Consumed
                }
                // Clipboard chords arrive here as `Character` KeyDowns because
                // the event loop promotes Ctrl/Cmd+letter out of `CharInput`.
                // (Ctrl/Cmd+V is intercepted upstream and replayed as
                // `CharInput`, so paste flows through the insert path and
                // replaces any selection via `delete_selection`.)
                Key::Character(c) if is_cmd_combo(ctx.modifiers) => {
                    match c.to_ascii_lowercase() {
                        'a' => {
                            self.select_all();
                            EventResult::Consumed
                        }
                        'c' => {
                            if let Some(text) = self.selected_text() {
                                ctx.write_clipboard(text);
                            }
                            EventResult::Consumed
                        }
                        'x' => {
                            // Cut = copy + delete the selection (a discrete
                            // undo step).
                            if let Some(text) = self.selected_text() {
                                ctx.write_clipboard(text);
                                self.begin_edit(EditKind::Delete, false);
                                if self.delete_selection() {
                                    self.commit_edit(EditKind::Delete, true, ctx);
                                }
                            }
                            EventResult::Consumed
                        }
                        // Undo / redo. Ctrl/Cmd+Z undoes; Ctrl/Cmd+Y redoes
                        // (the Windows convention). Ctrl/Cmd+Shift+Z also redoes
                        // — that chord carries Shift so it can't reach this
                        // `is_cmd_combo` arm; it's handled just above.
                        'z' => {
                            self.undo(ctx);
                            EventResult::Consumed
                        }
                        'y' => {
                            self.redo(ctx);
                            EventResult::Consumed
                        }
                        // Other Ctrl/Cmd+letter combos aren't ours — leave
                        // them for an app shortcut (the router already had
                        // first refusal upstream).
                        _ => EventResult::Ignored,
                    }
                }
                _ => EventResult::Ignored,
            },

            _ => EventResult::Ignored,
        };
        // Mirror the (possibly moved) caret + selection into the bound signals
        // so an external observer — e.g. a formatting toolbar — can read them.
        // No-op when the respective signal isn't bound.
        self.push_cursor_to_source();
        self.push_selection_to_source();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::word_bounds;

    #[test]
    fn word_bounds_grabs_word_from_middle() {
        // Caret inside "hello" expands to the whole word.
        assert_eq!(word_bounds("hello world", 2), (0, 5));
        assert_eq!(word_bounds("hello world", 8), (6, 11));
    }

    #[test]
    fn word_bounds_trailing_edge_prefers_word_over_gap() {
        // Caret between "hello" and the space: looking right would grab the
        // space, so we look left and select the word.
        assert_eq!(word_bounds("hello world", 5), (0, 5));
    }

    #[test]
    fn word_bounds_on_whitespace_selects_the_gap() {
        // Caret inside a run of spaces selects the run, not an adjacent word.
        assert_eq!(word_bounds("a    b", 3), (1, 5));
    }

    #[test]
    fn word_bounds_groups_punctuation_run() {
        // A run of punctuation is its own class.
        assert_eq!(word_bounds("a...b", 2), (1, 4));
    }

    #[test]
    fn word_bounds_does_not_cross_newline() {
        // `\n` is whitespace, so a word run stops at the line break.
        assert_eq!(word_bounds("foo\nbar", 5), (4, 7));
        assert_eq!(word_bounds("foo\nbar", 1), (0, 3));
    }

    #[test]
    fn word_bounds_respects_char_boundaries() {
        // Each kana is 3 UTF-8 bytes; the run is alphanumeric so all three are
        // selected, and the boundaries land on char starts (0 and 9).
        assert_eq!(word_bounds("あいう", 3), (0, 9));
    }

    #[test]
    fn word_bounds_at_end_of_text_looks_left() {
        assert_eq!(word_bounds("hello", 5), (0, 5));
    }

    #[test]
    fn word_bounds_empty_string_is_origin() {
        assert_eq!(word_bounds("", 0), (0, 0));
    }

    #[test]
    fn word_bounds_underscore_is_word_char() {
        // Identifiers with underscores select as one word.
        assert_eq!(word_bounds("foo_bar baz", 4), (0, 7));
    }
}
