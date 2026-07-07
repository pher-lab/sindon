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
//! widget's content width, ArrowUp / ArrowDown navigate by *visual* row
//! (following soft wraps, not just `\n`-delimited paragraphs) holding a
//! sticky x column, and the default height grows to [`Input::lines`]
//! line-heights. All other behavior (signal binding, placeholder, focus
//! ring, on_change) carries over unchanged.
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
use std::ops::Range;
use std::time::{Duration, Instant};

use crate::event::{EventContext, EventResult, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, FocusIndicator, Point, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::{Animated, Easing, Reactive, Signal};
use shroud_text::{FontWeight, TextAttrs, TextSpan};
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

/// A syntax-highlight classifier (B-1 spike). Given the current buffer, returns
/// the byte ranges to tint and the color for each — e.g. a keyword in blue, a
/// string literal in green. Ranges should be on `char` boundaries and not
/// overlap; the widget tiles the gaps between them with the default text color
/// (see [`build_highlight_spans`]). Called on every paint with the live buffer,
/// so the closure must be cheap and must not retain the `&str` it is handed (it
/// is the user's plaintext). `Fn` (not `FnMut`) because painting holds `&self`.
type Highlighter = Box<dyn Fn(&str) -> Vec<(usize, usize, Color)>>;

/// A smart-keymap hook (B-1 ③). Given the live buffer and the caret byte
/// offset, returns an optional [`KeyEdit`] to perform instead of the key's
/// default behavior — e.g. continuing a markdown list on Enter, or deleting a
/// whole list marker on Backspace. Returning `None` falls through to the
/// default. `Fn` (not `FnMut`): the hook is pure structural analysis of the
/// buffer it is handed, called from the event path while `&self` borrows the
/// buffer.
type KeymapHandler = Box<dyn Fn(&str, usize) -> Option<KeyEdit>>;

/// A structural edit produced by a smart-keymap hook ([`Input::on_enter`] /
/// [`Input::on_backspace`]). The widget applies it as one discrete undo step:
/// the byte range `replace` in the current buffer is replaced with `insert`,
/// then the caret moves to `caret` (a byte offset into the buffer *after* the
/// edit).
///
/// The widget validates the edit before applying it — `replace` must be a
/// well-formed, in-bounds range on `char` boundaries — and ignores a malformed
/// one (falling through to the key's default behavior) rather than panicking.
/// `caret` is clamped into the resulting buffer and snapped to a char boundary,
/// so an off-by-one in the hook can't crash the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEdit {
    /// Byte range in the *current* buffer to replace. For a pure insertion use
    /// an empty range at the caret (`cursor..cursor`).
    pub replace: Range<usize>,
    /// Replacement text spliced in where `replace` was.
    pub insert: String,
    /// Caret byte offset measured in the buffer *after* the edit is applied.
    pub caret: usize,
}

/// Tile `text` into color-only [`TextSpan`]s from a highlighter's colored
/// `ranges`, filling every gap with a default-attrs, no-color span so the spans
/// concatenate back to exactly `text`. That exact tiling is what keeps rich
/// rendering layout-identical to plain shaping — the spans differ from the plain
/// buffer only in color, and color never moves a glyph (proven in
/// `shroud_text/tests/highlight_layout_spike.rs`), so the caret / selection /
/// click geometry (all computed from plain shaping) stays valid.
///
/// Defensive: ranges are sorted, clamped to the buffer, and any that overlap a
/// previously-emitted range or fall on a non-`char` boundary are skipped rather
/// than panicking the paint-side slice — a misbehaving highlighter degrades to
/// less color, never a crash.
fn build_highlight_spans(
    text: &str,
    mut ranges: Vec<(usize, usize, Color)>,
    attrs: &TextAttrs,
) -> Vec<TextSpan> {
    ranges.sort_by_key(|&(lo, _, _)| lo);
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut pos = 0usize;
    // Every span carries the field's base attrs (family / weight / style) so a
    // highlighted field shapes at the same weight as the plain path — the
    // colored ranges only add color on top, keeping the geometry identical.
    for (lo, hi, color) in ranges {
        let lo = lo.min(text.len());
        let hi = hi.min(text.len());
        // Skip empty, out-of-order / overlapping, or boundary-splitting ranges.
        if lo < pos || lo >= hi || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
            continue;
        }
        if lo > pos {
            spans.push(TextSpan::new(text[pos..lo].to_string()).attrs(attrs.clone()));
        }
        spans.push(
            TextSpan::new(text[lo..hi].to_string())
                .attrs(attrs.clone())
                .color(color),
        );
        pos = hi;
    }
    if pos < text.len() {
        spans.push(TextSpan::new(text[pos..].to_string()).attrs(attrs.clone()));
    }
    spans
}

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

/// Width of the multi-line viewport's scrollbar indicator (px). Mirrors
/// `ScrollView`'s scrollbar metrics so the two controls read as one.
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Inset of the scrollbar from the field's inner right edge (px).
const SCROLLBAR_INSET: f32 = 2.0;
/// Minimum thumb height (px) so the handle stays grabbable on very long content.
const SCROLLBAR_THUMB_MIN: f32 = 16.0;
/// Horizontal lane reserved on the right of a multi-line field for the
/// scrollbar overlay. Subtracted from the wrap width so text wraps — and the
/// caret stops — *before* the bar instead of being drawn under it (#34).
/// Covers the bar's width plus its inset from the inner right edge.
const SCROLLBAR_LANE: f32 = SCROLLBAR_WIDTH + SCROLLBAR_INSET;

/// Glide duration for wheel scrolling the multi-line viewport. Matches
/// `ScrollView`'s default (FW-7) so the two scroll surfaces feel identical;
/// caret-reveal and re-clamp bypass it via `Animated::snap`.
const SCROLL_TRANSITION: Duration = Duration::from_millis(120);

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
///
/// The CJK scripts get their own classes (Han / Hiragana / Katakana) instead of
/// folding into `Word`. Japanese has no spaces between words, so treating every
/// ideograph and kana as one `Word` run made a double-click grab the whole
/// sentence (FW-11). True segmentation needs a dictionary (MeCab-class), which
/// is too heavy for a zeroize-first framework; splitting on *script* runs is the
/// dictionary-free approximation — a double-click in 日本語 grabs the kanji run
/// and stops at an adjacent kana. It is "less wrong", and pairs with the
/// triple-click line select (FW-10) as the practical fallback.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    /// Alphanumeric or `_` that isn't one of the segmented CJK scripts below
    /// (Latin, digits, Cyrillic, Hangul, …). These scripts are space-delimited,
    /// so a run still stops at the next space as before.
    Word,
    /// CJK ideographs (Han), incl. the ideographic iteration mark `々`.
    Han,
    /// Hiragana.
    Hiragana,
    /// Katakana, incl. the prolonged sound mark `ー` and halfwidth katakana.
    Katakana,
    /// Whitespace (including `\n`, so word runs never cross a hard line break).
    Space,
    /// Everything else (punctuation, symbols).
    Other,
}

fn classify(c: char) -> CharClass {
    // CJK scripts are checked before the generic `is_alphanumeric` bucket: CJK
    // characters *are* alphanumeric, so the specific ranges must win to keep
    // each script its own run (FW-11).
    if c.is_whitespace() {
        CharClass::Space
    } else if is_hiragana(c) {
        CharClass::Hiragana
    } else if is_katakana(c) {
        CharClass::Katakana
    } else if is_han(c) {
        CharClass::Han
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// Hiragana block (letters, small kana, iteration marks, combining sound marks).
fn is_hiragana(c: char) -> bool {
    matches!(u32::from(c), 0x3040..=0x309F)
}

/// Katakana: the main block (incl. the prolonged sound mark `ー`, U+30FC),
/// phonetic extensions, and the halfwidth katakana used by legacy input.
fn is_katakana(c: char) -> bool {
    matches!(u32::from(c), 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F)
}

/// CJK ideographs: the Unified block plus Extension A, compatibility ideographs,
/// the supplementary-plane extensions, and the ideographic iteration / number
/// marks (`々 〆 〇`) that read as part of a kanji run.
fn is_han(c: char) -> bool {
    matches!(
        u32::from(c),
        0x3005..=0x3007        // 々 〆 〇
        | 0x3400..=0x4DBF      // Extension A
        | 0x4E00..=0x9FFF      // Unified Ideographs
        | 0xF900..=0xFAFF      // Compatibility Ideographs
        | 0x2_0000..=0x2_FA1F  // Supplementary (Ext B–F + compat supplement)
    )
}

/// Granularity a pending pointer selection expands to, resolved against the
/// shaped text at paint. A single press plants a bare caret; a double-click
/// grabs the word under it ([`word_bounds`]); a triple-click grabs the whole
/// logical line ([`line_bounds`]). Tracked as a unit (not a bool) because the
/// precise hit-test — and the expansion — are deferred to paint, where the text
/// engine is in hand (see [`Input::pending_hit`]).
#[derive(PartialEq, Eq, Clone, Copy)]
enum SelectUnit {
    Caret,
    Word,
    Line,
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

/// Expand a caret `offset` to the byte range of the logical line under it, used
/// by triple-click selection. A "logical line" is the run between hard line
/// breaks (`\n`); the returned range covers the line's *content* with the
/// trailing newline excluded, so replacing the selection edits the line in
/// place rather than swallowing the break and merging it with the next. The
/// bounds land on `\n` (or the buffer edges), which are always char boundaries,
/// so multibyte text is safe. `offset` is clamped and treated as a char
/// boundary — the same contract as [`word_bounds`].
fn line_bounds(s: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(s.len());
    let lo = s[..offset].rfind('\n').map_or(0, |i| i + 1);
    let hi = s[offset..].find('\n').map_or(s.len(), |i| offset + i);
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
    /// Timestamp, position, and resulting count of the last primary press, used
    /// to chain a multi-click. A press within [`DOUBLE_CLICK_MAX`] and
    /// [`DOUBLE_CLICK_SLOP`] of the previous one advances the count (1 → 2 → 3,
    /// then cycles back to 1), promoting the pending hit to a word (2) or whole
    /// line (3) select; a press that lands too late or too far starts fresh at 1.
    last_click: Cell<Option<(Instant, Point, u8)>>,
    /// Granularity the next deferred hit resolves to, set on press from the
    /// multi-click count and consumed at paint to expand the caret offset to the
    /// surrounding word ([`word_bounds`]) or logical line ([`line_bounds`]).
    pending_select: Cell<SelectUnit>,
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
    /// "Sticky" target *x* (px, in the text's local space) tracked across
    /// ArrowUp / ArrowDown so vertical navigation through ragged / wrapped
    /// lines lands the caret at the same visual column it started from — even
    /// when it passes through shorter lines. Seeded from the caret's x on the
    /// first vertical move of a run and reused by later ones. Cleared by any
    /// non-vertical edit or motion. An x (not a char column) is what makes the
    /// caret track *visual* rows correctly through soft wraps and variable
    /// glyph widths.
    desired_x: Cell<Option<f32>>,
    /// Net vertical move (visual rows) requested by ArrowUp/ArrowDown and not
    /// yet resolved: negative = up, positive = down. `Input::event` has no text
    /// engine, so — like [`pending_hit`](Self::pending_hit) — the move is
    /// deferred to `paint`, which walks it one wrapped row at a time against the
    /// shaped layout. Accumulates so a burst of key-repeat presses arriving
    /// before the next paint are not dropped.
    pending_vmove: Cell<i32>,
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
    bg_color: Option<Reactive<Color>>,
    text_color: Option<Reactive<Color>>,
    placeholder_color: Option<Reactive<Color>>,
    border_color: Option<Reactive<Color>>,
    /// Whether to draw the 1px border at all. `false` (via [`borderless`]) skips
    /// the stroke, leaving just the (optionally rounded) background fill — handy
    /// for inline / search-bar styling. [`borderless`]: Self::borderless
    border_visible: bool,
    /// Corner radius (px) for the background fill and border stroke. `0.0` keeps
    /// the historical sharp rectangle and short-circuits the SDF in the shader.
    radius: f32,
    /// Horizontal text inset (px) between the border and the text, on each
    /// side. Default 8. Feeds the layout padding, the caret / hit-test
    /// geometry, and the wrap width. Maps to Tailwind `px-*`.
    pad_x: f32,
    /// Vertical text inset (px). Default 8. In single-line mode the text is
    /// centered so this only feeds the derived `min_height`; in multi-line mode
    /// it is the top / bottom inset of the scrolling viewport. Maps to Tailwind
    /// `py-*`.
    pad_y: f32,
    /// Explicit `min_height` (px) override for the field's box. `None` derives
    /// it from the font size (single-line) or line count (multi-line). Lets an
    /// app match a design's exact control height (e.g. a 36px search bar).
    min_height_override: Option<f32>,
    focus_ring_color: Option<Reactive<Color>>,
    /// Override for the selection highlight color. `None` reads
    /// `theme.colors.selection_background` each frame.
    selection_color: Option<Reactive<Color>>,
    /// Font family / weight / style the value + placeholder shape with. Defaults
    /// to the plain sans-serif so existing fields are unaffected; the caret,
    /// hit-test, and selection geometry all shape through this so a heavier
    /// [`weight`](Self::weight) stays aligned with the painted glyphs.
    attrs: TextAttrs,
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
    /// Optional syntax-highlight classifier (B-1 spike). When `Some`, the
    /// non-empty render path tiles the buffer into color-only spans via this
    /// closure and shapes them with `shape_rich` instead of the plain
    /// `shape_text`. Color-only spans are layout-identical to plain shaping, so
    /// the (plain-shaped) caret / selection geometry is unaffected.
    highlighter: Option<Highlighter>,
    /// Smart-keymap hook for Enter in multi-line mode (B-1 ③). When `Some` and
    /// there is no active selection, Enter consults this for a structural edit
    /// (e.g. continue a markdown list) before falling back to a plain newline.
    enter_handler: Option<KeymapHandler>,
    /// Smart-keymap hook for Backspace (B-1 ③). When `Some`, a no-selection
    /// Backspace with `cursor > 0` consults this for a structural edit (e.g.
    /// delete a whole list marker) before falling back to the single-char
    /// delete. Independent of [`on_backspace_empty`](Self::on_backspace_empty),
    /// which only fires on an empty buffer.
    backspace_handler: Option<KeymapHandler>,
    /// Internal vertical scroll for multi-line mode (px). The *target* offset —
    /// where wheel input, caret-reveal, and clamping point — lives here; the
    /// displayed offset eases toward it so a wheel flick glides instead of
    /// teleporting (FW-7b). Glyphs are clipped to the padding box and shifted up
    /// by the *displayed* value. `Animated` (interior-mutable) so paint (which
    /// holds `&self`) can retarget / snap it. Rests at 0 in single-line mode.
    /// Wheel uses `set` (eased); caret-reveal and re-clamp use `snap` (instant).
    scroll_anim: Animated<f32>,
    /// Set whenever the caret moves (edit, arrow, click, undo) so the next paint
    /// scrolls the viewport to keep the caret visible. Taken (cleared) by paint.
    /// A wheel scroll deliberately does *not* set it, so scrolling away from the
    /// caret with the mouse sticks instead of snapping back.
    reveal_caret: Cell<bool>,
    /// Max scroll offset (`content_h - viewport_h`, clamped at 0) computed at the
    /// last paint. Read by the wheel handler — which has no text engine to
    /// re-measure — to decide whether the field has anything to scroll (and thus
    /// whether to consume the wheel). Paint re-clamps authoritatively.
    last_max_scroll: Cell<f32>,
    /// Flex-grow factor applied in [`Widget::style`] (`None` = no grow).
    grow: Option<f32>,
    /// When set, the field fills its parent's height (`height: 100%`). In
    /// multi-line mode this turns it into a fixed viewport that scrolls its
    /// content internally rather than sizing to a fixed [`line_count`].
    fill_height: bool,
    /// Explicit fixed box height (px), CSS `h-24`. When `Some`, the field is
    /// exactly this tall regardless of content or parent — a *definite*
    /// viewport that (in multi-line mode) clips and scrolls overflow instead
    /// of growing. Takes precedence over the derived `min_height` floor and
    /// over [`fill_height`](Self::fill_height). `None` = size by
    /// `min_height` / `line_count` / `fill_height` as before.
    fixed_height: Option<f32>,
    /// IME preedit (composition) text — the uncommitted characters the user
    /// is currently composing via an IME. Spliced into the buffer *for
    /// display only* at the caret (with an underline) so the user can see
    /// what they're typing; it never enters [`value`](Self::value) until the
    /// IME commits, which arrives as a burst of [`CharInput`]. Empty when not
    /// composing. Set from [`WidgetEvent::ImePreedit`] and cleared on commit
    /// (empty preedit) or `FocusLost`.
    ///
    /// [`CharInput`]: crate::event::WidgetEvent::CharInput
    /// [`WidgetEvent::ImePreedit`]: crate::event::WidgetEvent::ImePreedit
    preedit: String,
    /// Caret byte range within [`preedit`](Self::preedit) reported by the IME
    /// (`None` = caret hidden during composition). The displayed caret sits at
    /// `cursor + preedit_cursor.start`, so it tracks the composition's own
    /// cursor rather than jumping to the end of the preedit.
    preedit_cursor: Option<(usize, usize)>,
}

impl Input {
    /// Create a new empty input.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(String::new()),
            cursor: Cell::new(0),
            selection_anchor: Cell::new(None),
            pending_hit: Cell::new(None),
            pending_select: Cell::new(SelectUnit::Caret),
            selecting: Cell::new(false),
            last_click: Cell::new(None),
            source: None,
            cursor_source: None,
            selection_source: None,
            placeholder: String::new(),
            font_size: None,
            focused: false,
            multiline: false,
            line_count: None,
            desired_x: Cell::new(None),
            pending_vmove: Cell::new(0),
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
            border_visible: true,
            radius: 0.0,
            pad_x: 8.0,
            pad_y: 8.0,
            min_height_override: None,
            focus_ring_color: None,
            selection_color: None,
            attrs: TextAttrs::default(),
            undo_stack: RefCell::new(VecDeque::new()),
            redo_stack: RefCell::new(VecDeque::new()),
            last_edit: Cell::new(None),
            highlighter: None,
            enter_handler: None,
            backspace_handler: None,
            scroll_anim: Animated::new(0.0, SCROLL_TRANSITION, Easing::EaseOut),
            reveal_caret: Cell::new(false),
            last_max_scroll: Cell::new(0.0),
            grow: None,
            fill_height: false,
            fixed_height: None,
            preedit: String::new(),
            preedit_cursor: None,
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
    /// content width, and ArrowUp / ArrowDown navigate by *visual* row —
    /// following soft wraps, not just `\n`-delimited paragraphs — holding a
    /// sticky x column (so passing through a short row keeps the caret's
    /// column), and snapping to the buffer start / end at the top / bottom
    /// edge. Tab is *not* captured — the focus manager keeps owning it — so
    /// the field stays a friendly form citizen.
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

    /// Flex-grow factor — the field claims this share of leftover main-axis
    /// space from its parent. Pair with [`multiline`](Self::multiline) +
    /// [`height_full`](Self::height_full) for an editor that fills the pane and
    /// scrolls its content internally instead of growing without bound.
    pub fn grow(mut self, factor: f32) -> Self {
        self.grow = Some(factor);
        self
    }

    /// Fill the parent's height (`height: 100%`).
    ///
    /// In [`multiline`](Self::multiline) mode this turns the field into a fixed
    /// viewport: content taller than the viewport scrolls internally (mouse
    /// wheel, and the caret is auto-revealed on edit / navigation) rather than
    /// the field growing past its box. Without it a multi-line field sizes to
    /// [`lines`](Self::lines) line-heights. No effect in single-line mode.
    pub fn height_full(mut self) -> Self {
        self.fill_height = true;
        self
    }

    /// Set a fixed box height in pixels (CSS `h-24` on a `<textarea>`).
    ///
    /// Unlike [`min_height`](Self::min_height) (a floor the field grows past
    /// as content is added) and [`height_full`](Self::height_full) (fills the
    /// parent), this pins the field to *exactly* `px` tall regardless of
    /// content or parent size. In [`multiline`](Self::multiline) mode that
    /// makes it a definite viewport: content taller than the box clips and
    /// scrolls internally (wheel + caret auto-reveal), matching a Tailwind
    /// `resize-none h-24` textarea. In single-line mode it simply fixes the
    /// box height with the text vertically centered.
    ///
    /// Takes precedence over the derived `min_height` floor and over
    /// [`height_full`](Self::height_full).
    pub fn height(mut self, px: f32) -> Self {
        self.fixed_height = Some(px.max(0.0));
        self
    }

    /// Set how long a wheel scroll takes to glide to its new position in a
    /// multi-line viewport. Defaults to `SCROLL_TRANSITION` (120 ms), matching
    /// `ScrollView`; pass [`Duration::ZERO`] for the pre-animation instant jump
    /// (FW-7b). Caret-reveal and re-clamp always snap regardless of this value.
    pub fn scroll_transition(mut self, duration: Duration) -> Self {
        // Recreate the animator with the new duration (it is fixed at
        // construction). Builders run before any scroll, so resetting to 0 is
        // safe.
        self.scroll_anim = Animated::new(0.0, duration, Easing::EaseOut);
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

    /// Set the background color. Accepts a literal `Color` or a signal-backed
    /// source (`Signal<Color>`, `Memo<Color>`, `Reactive::derive(...)`), re-read
    /// every paint so an explicit background can track a live theme swap.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.bg_color = Some(color.into());
        self
    }

    /// Override the 1px border color. `None` (the default) reads
    /// `theme.colors.input_border` each frame, so every input tracks the theme;
    /// set this to give one field a distinct frame. Has no effect once
    /// [`borderless`](Self::borderless) is used. [`Reactive`], re-read every
    /// paint like [`background`](Self::background).
    pub fn border_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.border_color = Some(color.into());
        self
    }

    /// Set the font weight the value shapes with (e.g. `FontWeight::BOLD`).
    /// Default is `FontWeight::NORMAL`. The caret, hit-test, and selection
    /// geometry all shape through the same weight, so a bold field stays
    /// editable with the caret tracking the (wider) glyphs — the missing piece
    /// for a `text-2xl font-bold` title field.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.attrs.weight = weight;
        self
    }

    /// Drop the border entirely, leaving just the background fill. Useful for
    /// inline editing or a search-bar look where a boxed frame would feel heavy.
    pub fn borderless(mut self) -> Self {
        self.border_visible = false;
        self
    }

    /// Round the corners of the background fill and border by `px`. `0.0` (the
    /// default) keeps the sharp rectangle. Symmetric with
    /// [`Container::radius`](crate::Container::radius) /
    /// [`Button::radius`](crate::Button::radius). Negative values clamp to `0.0`;
    /// over-large values are clamped to half the shorter side in the renderer.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
        self
    }

    /// Horizontal text inset (px) — the padding between the field's border and
    /// the text, on each side. Default 8. Maps to Tailwind `px-*`. Widens (or
    /// tightens) the caret / hit-test geometry and the multi-line wrap width to
    /// match, so a boxed field can carry the same generous inset as its design.
    /// Negative values clamp to `0.0`.
    pub fn padding_x(mut self, px: f32) -> Self {
        self.pad_x = px.max(0.0);
        self
    }

    /// Vertical text inset (px). Default 8. Maps to Tailwind `py-*`. In
    /// single-line mode the text is centered, so this only grows the derived
    /// [`min_height`](Self::min_height); in multi-line mode it is the top /
    /// bottom inset of the internally-scrolling viewport. Negative values clamp
    /// to `0.0`.
    pub fn padding_y(mut self, px: f32) -> Self {
        self.pad_y = px.max(0.0);
        self
    }

    /// Explicit minimum box height (px), overriding the value derived from font
    /// size (single-line) or line count (multi-line). Lets an app match a
    /// design's exact control height — e.g. a compact 36px search bar or a
    /// `py-3` ≈48px field — instead of the built-in floor. Negative values
    /// clamp to `0.0`.
    pub fn min_height(mut self, px: f32) -> Self {
        self.min_height_override = Some(px.max(0.0));
        self
    }

    /// Set the text color. [`Reactive`], re-read every paint so it can track a
    /// live theme swap.
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    /// Override the keyboard-focus ring color. `None` (the default) reads
    /// `theme.focus.ring_color` each frame. [`Reactive`], like the other color
    /// setters.
    pub fn focus_ring_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.focus_ring_color = Some(color.into());
        self
    }

    /// Override the text-selection highlight color. `None` (the default)
    /// reads `theme.colors.selection_background` each frame. A translucent
    /// color keeps the selected glyphs legible on top. [`Reactive`], like the
    /// other color setters.
    pub fn selection_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.selection_color = Some(color.into());
        self
    }

    /// Attach a syntax-highlight classifier (B-1 spike, experimental).
    ///
    /// The closure is called on every paint with the current buffer and returns
    /// `(start, end, color)` byte ranges to tint; gaps render in the default
    /// text color. Because the spans differ from the plain buffer only in color
    /// — and color never moves a glyph — the caret, selection, and click
    /// hit-testing (all computed from plain shaping) stay correct without any
    /// rich-aware geometry. Ranges must be on `char` boundaries and not overlap;
    /// malformed ranges are skipped, never panicked on.
    ///
    /// The closure receives the user's plaintext, so it must not retain it. Has
    /// no effect on `SecureInput`, which keeps no highlighter (a secret must not
    /// be classified into spans). Pairs naturally with [`multiline`](Self::multiline)
    /// for a code editor.
    pub fn highlighter(mut self, f: impl Fn(&str) -> Vec<(usize, usize, Color)> + 'static) -> Self {
        self.highlighter = Some(Box::new(f));
        self
    }

    /// Intercept Enter in [`multiline`](Self::multiline) mode with a smart-keymap
    /// hook (B-1 ③).
    ///
    /// When set, pressing Enter with no active selection calls the closure with
    /// the current buffer and caret byte offset. If it returns a [`KeyEdit`],
    /// that edit is applied as one discrete undo step instead of inserting a
    /// plain newline; returning `None` — or a malformed edit — falls through to
    /// the newline. Typical use is continuing a markdown list / blockquote on
    /// the next line, or clearing an empty list item. No effect in single-line
    /// mode (where Enter fires [`on_submit`](Self::on_submit)) or while text is
    /// selected.
    ///
    /// The closure receives the user's plaintext, so it must not retain it — the
    /// same posture as [`highlighter`](Self::highlighter). Pairs naturally with
    /// [`on_backspace`](Self::on_backspace) for a markdown editor.
    pub fn on_enter(mut self, f: impl Fn(&str, usize) -> Option<KeyEdit> + 'static) -> Self {
        self.enter_handler = Some(Box::new(f));
        self
    }

    /// Intercept Backspace with a smart-keymap hook (B-1 ③).
    ///
    /// When set, pressing Backspace with no selection and a non-empty prefix
    /// (`cursor > 0`) calls the closure with the current buffer and caret byte
    /// offset. If it returns a [`KeyEdit`], that edit is applied as one discrete
    /// undo step instead of deleting the single preceding character; `None` — or
    /// a malformed edit — falls through to the single-char delete (which still
    /// coalesces with a run of Backspaces). Typical use is deleting a whole
    /// markdown list marker in one stroke.
    ///
    /// Distinct from [`on_backspace_empty`](Self::on_backspace_empty), which
    /// fires only on an *empty* buffer and produces no edit (it lets a tag
    /// editor remove the last chip). The two never overlap — this hook requires
    /// `cursor > 0`, that one requires an empty buffer.
    ///
    /// The closure receives the user's plaintext, so it must not retain it.
    pub fn on_backspace(mut self, f: impl Fn(&str, usize) -> Option<KeyEdit> + 'static) -> Self {
        self.backspace_handler = Some(Box::new(f));
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
        self.desired_x.set(None);
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
            self.desired_x.set(None);
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
    /// no clamping needed. `desired_x` is dropped (vertical-nav state is not
    /// part of the undo model).
    fn apply_snapshot(&self, snap: &Snapshot) {
        self.value.borrow_mut().clear();
        self.value.borrow_mut().push_str(&snap.text);
        self.cursor.set(snap.cursor);
        self.selection_anchor.set(snap.anchor);
        self.desired_x.set(None);
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

    /// Apply a [`KeyEdit`] from a smart-keymap hook as one discrete undo step:
    /// splice `insert` over the `replace` range and move the caret to `caret`
    /// (clamped + char-boundary-snapped against the resulting buffer). Returns
    /// `false` — mutating nothing — when the edit's range is reversed, out of
    /// bounds, or splits a multi-byte char, so the caller can fall through to
    /// the key's default behavior. A misbehaving hook degrades; it never panics
    /// the editor. `kind` only tags the (non-coalescing) undo step.
    fn apply_key_edit(&mut self, kind: EditKind, edit: KeyEdit, ctx: &mut EventContext) -> bool {
        let KeyEdit {
            replace,
            insert,
            caret,
        } = edit;
        {
            let buf = self.value.borrow();
            if replace.start > replace.end
                || replace.end > buf.len()
                || !buf.is_char_boundary(replace.start)
                || !buf.is_char_boundary(replace.end)
            {
                return false;
            }
        }
        // A structural edit is its own discrete undo step — like a typed newline
        // or a selection replace, it never coalesces with surrounding typing.
        self.begin_edit(kind, false);
        self.value.borrow_mut().replace_range(replace, &insert);
        // Clamp the requested caret into the new buffer and snap it down to a
        // char boundary so a miscounted offset can't panic the paint-side slice.
        let mut target = caret.min(self.value.borrow().len());
        {
            let buf = self.value.borrow();
            while target > 0 && !buf.is_char_boundary(target) {
                target -= 1;
            }
        }
        self.cursor.set(target);
        self.selection_anchor.set(None);
        self.desired_x.set(None);
        self.commit_edit(kind, true, ctx);
        true
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
        self.bg_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.input_background)
    }

    fn resolve_border(&self, colors: &shroud_core::Colors) -> Color {
        self.border_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.input_border)
    }

    /// Color of the focus indicator (ring in `Ring` mode, focused border in
    /// `Border` mode): the per-widget `focus_ring_color` override if set,
    /// else the theme's `focus.ring_color`. One source keeps the two modes
    /// in agreement and makes the existing override work for both.
    fn focus_indicator_color(&self, focus: &shroud_core::FocusStyle) -> Color {
        self.focus_ring_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(focus.ring_color)
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
        // Whether the caller is *also* setting a fresh caret / selection this
        // sync — a deliberate programmatic move (a toolbar insert, a
        // find-replace jump) rather than a passive rebase. A note switch leaves
        // these signals untouched, so they still equal what the widget last
        // reported (the tail of every event mirrors them out), and this stays
        // `false`. It drives two things below: a deliberate move keeps the
        // scroll position and reveals the new caret, whereas a passive rebase
        // snaps to the top.
        let caret_incoming = self
            .cursor_source
            .as_ref()
            .is_some_and(|c| c.get() != self.cursor.get())
            || self
                .selection_source
                .as_ref()
                .is_some_and(|s| s.get() != self.selection_range());

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
                // A note switch should show the new body from the top rather
                // than inheriting the previous note's scroll offset — but only
                // for a passive rebase. When the caller moves the caret in the
                // same update (a toolbar insert, a find-replace replacement),
                // the caret-reveal below positions the viewport instead, so a
                // mid-document edit doesn't snap back to the top.
                if !caret_incoming {
                    // Snap (not glide): a note switch should appear at the top
                    // immediately, never scroll up from the old position.
                    self.scroll_anim.snap(0.0);
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

        // A programmatic caret / selection move (find-replace jumping to a
        // match, the toolbar repositioning after an insert) should scroll into
        // view on the next paint, exactly like a keyboard caret move does. The
        // paint-side scroll-to-caret honors this flag only while focused +
        // multi-line, so setting it here is inert for single-line / unfocused
        // fields.
        if caret_incoming {
            self.reveal_caret.set(true);
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
        let (pad_x, pad_y) = (self.pad_x, self.pad_y);
        let base = FlexStyle::new().padding_trbl(pad_y, pad_x, pad_y, pad_x);
        // A fixed `height(px)` is a definite viewport that owns its own size —
        // it supersedes the derived `min_height` floor (and `fill_height`
        // below), so overflow clips and scrolls instead of growing the box.
        let mut style = if let Some(h) = self.fixed_height {
            base.height(h)
        } else if self.multiline {
            let line_height = font_size * 1.2;
            // When the field fills its parent's height it owns a viewport that
            // scrolls internally, so a tiny floor (2 rows) is enough to keep it
            // usable in a small pane; the explicit `lines` count is only the
            // size when *not* filling.
            let rows = if self.fill_height {
                2.0
            } else {
                self.line_count.unwrap_or(3) as f32
            };
            // Default floor = rows of text plus the top/bottom insets (so a
            // taller `padding_y` grows the box); `min_height` overrides it.
            let derived = rows * line_height + 2.0 * pad_y;
            base.min_height(self.min_height_override.unwrap_or(derived))
        } else {
            // `+ 4.0` keeps a hair of breathing room around the centered line,
            // preserving the historical `font_size + 20` at the default pad 8.
            let derived = font_size + 2.0 * pad_y + 4.0;
            base.min_height(self.min_height_override.unwrap_or(derived))
        };
        if self.fill_height && self.fixed_height.is_none() {
            style = style.height_full();
        }
        if let Some(factor) = self.grow {
            style = style.grow(factor);
        }
        style
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        self.sync_from_source();

        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = font_size * 1.2;
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(ctx.theme.colors.on_surface);
        let placeholder_color = self
            .placeholder_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(ctx.theme.colors.input_placeholder);
        let bg = self.resolve_bg(&ctx.theme.colors);

        // Background fill. `radius == 0.0` short-circuits the SDF, so this is the
        // historical sharp rect unless the app opted into rounded corners.
        ctx.fill_rect_rounded(layout, bg, self.radius);

        // Focus indicator: whether this frame should signal focus, and whether
        // the theme's `Border` mode can express it here (needs a visible border
        // to recolor). `border_focus` recolors the stroke below and suppresses
        // the ring; otherwise the ring paints (Ring mode, or Border-mode
        // fallback when the field is borderless).
        let focus_active = self.focused && ctx.focus_visible();
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
            ctx.stroke_rect_rounded(layout, border, self.radius, 1.0);
        }

        let text_x = layout.origin.x + self.pad_x;
        // Single-line: vertically center the (one) line of text so the field
        // looks balanced. Multi-line: top-align to padding so the block grows
        // downward as the user types and ArrowUp/Down stay predictable.
        let text_y = if self.multiline {
            layout.origin.y + self.pad_y
        } else {
            layout.origin.y + (layout.size.height - font_size) / 2.0
        };
        let max_width = layout.size.width - 2.0 * self.pad_x;
        // Pass max_width as the wrap constraint only in multi-line mode.
        // Single-line inputs intentionally let text overflow horizontally
        // (and don't draw outside their bounds because the wgpu pipeline
        // clips to the widget — adding wrap here would unexpectedly stack
        // text into multiple visual rows inside a one-line input).
        //
        // Reserve the scrollbar lane on the right so a full line wraps before
        // the bar's column and a caret at the line end never lands under the
        // overlay (#34). Always reserved (not only when the bar is visible) so
        // the wrap width — which the caret, hit-test and selection geometry all
        // key off — stays stable instead of jumping when the bar appears.
        let wrap_width = if self.multiline {
            Some((max_width - SCROLLBAR_LANE).max(0.0))
        } else {
            None
        };

        // IME preedit: while composing (focused + non-empty preedit), splice the
        // uncommitted composition text into the buffer *for display only* at the
        // caret. The real `value` is untouched until the IME commits. All the
        // shaping below (content height, caret geometry, glyphs) runs on this
        // display string so the composition is visible and the caret tracks it;
        // the committed text is what later lands in `value` via CharInput.
        //
        // `composed_caret` is the byte offset of the displayed caret within the
        // composed string (the IME's own cursor inside the preedit, defaulting
        // to its end). `preedit_span` is the composed-string byte range the
        // preedit occupies, used to underline it.
        let composing = self.focused && !self.preedit.is_empty();
        let mut composed_text: Option<String> = None;
        let mut composed_caret = 0usize;
        let mut preedit_span: Option<(usize, usize)> = None;
        if composing {
            let v = self.value.borrow();
            let cur = self.cursor.get().min(v.len());
            let mut s = String::with_capacity(v.len() + self.preedit.len());
            s.push_str(&v[..cur]);
            s.push_str(&self.preedit);
            s.push_str(&v[cur..]);
            let span = (cur, cur + self.preedit.len());
            composed_caret = match self.preedit_cursor {
                Some((cs, _ce)) => cur + cs.min(self.preedit.len()),
                None => span.1,
            };
            preedit_span = Some(span);
            composed_text = Some(s);
        }

        // Multi-line internal viewport: shape the buffer once to learn its
        // content height, clamp the stored scroll offset against it, and (after
        // the hit-test below resolves any click) scroll so a just-moved caret
        // stays visible. Single-line fields never scroll (`scroll_y` stays 0).
        // While composing, the (non-empty) preedit means there is always
        // something to draw, so the placeholder path is suppressed.
        let value_is_empty = self.value.borrow().is_empty() && !composing;
        let viewport_h = (layout.size.height - 2.0 * self.pad_y).max(0.0);
        // `displayed` is the eased offset the text is actually drawn at; the
        // logical target lives in `scroll_anim`. Hit-testing, the offset push,
        // and the scrollbar all use `displayed` so they match what is on screen
        // while a wheel glide is mid-flight (FW-7b).
        let mut displayed = 0.0_f32;
        let mut max_scroll = 0.0_f32;
        if self.multiline {
            let content_h = if value_is_empty {
                line_height
            } else if let Some(ct) = composed_text.as_deref() {
                ctx.text_engine
                    .shape_text_attrs(ct, font_size, line_height, wrap_width, &self.attrs)
                    .height
            } else {
                let v = self.value.borrow();
                ctx.text_engine
                    .shape_text_attrs(&v, font_size, line_height, wrap_width, &self.attrs)
                    .height
            };
            max_scroll = (content_h - viewport_h).max(0.0);
            // Re-clamp the target against the freshly measured content. A shrink
            // (shorter note, deleted lines) snaps instantly — no slide — while a
            // still-valid target leaves any in-flight wheel glide running.
            let clamped = self.scroll_anim.target().clamp(0.0, max_scroll);
            if clamped != self.scroll_anim.target() {
                self.scroll_anim.snap(clamped);
            }
            displayed = self.scroll_anim.get();
        }

        // Resolve a deferred click / drag against the shaped text now that the
        // engine + geometry are in hand (the event handler has neither — see
        // `pending_hit`). `extend` keeps the anchor and moves only the active
        // end; otherwise the caret collapses to a fresh, selection-free point.
        if let Some((pos, extend)) = self.pending_hit.take() {
            let rel_x = pos.x - text_x;
            // Add the displayed scroll offset so a click maps to the character
            // the user actually sees (the text is drawn shifted up by it).
            let rel_y = pos.y - text_y + displayed;
            let offset = {
                let v = self.value.borrow();
                ctx.text_engine.offset_at_point_attrs(
                    &v,
                    rel_x,
                    rel_y,
                    font_size,
                    line_height,
                    wrap_width,
                    &self.attrs,
                )
            };
            match self.pending_select.replace(SelectUnit::Caret) {
                // Double- / triple-click: expand the resolved caret to the
                // surrounding word or whole logical line.
                unit @ (SelectUnit::Word | SelectUnit::Line) => {
                    let (lo, hi) = {
                        let v = self.value.borrow();
                        match unit {
                            SelectUnit::Word => word_bounds(&v, offset),
                            _ => line_bounds(&v, offset),
                        }
                    };
                    self.selection_anchor.set(Some(lo));
                    self.cursor.set(hi);
                }
                SelectUnit::Caret => {
                    if extend {
                        self.ensure_anchor();
                    } else {
                        self.selection_anchor.set(None);
                    }
                    self.cursor.set(offset);
                }
            }
            self.desired_x.set(None);
            // A click / drag moves the caret outside the edit path, so end any
            // in-progress coalescing run — the next keystroke starts a fresh
            // undo step rather than merging with text typed before the click.
            self.last_edit.set(None);
            // Mirror the moved caret + selection so bound signals (and the next
            // event's `sync_from_source`) agree with the paint-resolved hit.
            self.push_cursor_to_source();
            self.push_selection_to_source();
        }

        // Resolve any deferred vertical move (ArrowUp/Down). Like the click
        // above, this needs the engine to map the caret to a visual (x, y) and
        // back, so `event` only accumulates a net row delta and we walk it here
        // against the *wrapped* layout — the caret follows soft-wrapped visual
        // rows, not `\n`-delimited paragraphs (the FW-2 fix). `desired_x` is the
        // sticky column: seeded from the caret on the first step of a run and
        // reused after, so passing through a short row doesn't drag it left.
        let vmove = self.pending_vmove.replace(0);
        if vmove != 0 {
            let down = vmove > 0;
            let mut steps = vmove.unsigned_abs();
            while steps > 0 {
                let cursor = self.cursor.get();
                let (cx, cy) = {
                    let v = self.value.borrow();
                    ctx.text_engine.caret_at_offset_attrs(
                        &v,
                        cursor,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
                };
                let target_x = self.desired_x.get().unwrap_or(cx);
                self.desired_x.set(Some(target_x));
                let new_cursor = if down {
                    // One visual row down at the sticky column. A `y` past the
                    // last row makes `offset_at_point` return the buffer end —
                    // exactly the wanted "ArrowDown on the last line jumps to
                    // end" behavior.
                    let v = self.value.borrow();
                    ctx.text_engine.offset_at_point_attrs(
                        &v,
                        target_x,
                        cy + line_height,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
                } else if cy < line_height {
                    // Already on the first visual row: ArrowUp jumps to start.
                    0
                } else {
                    let v = self.value.borrow();
                    ctx.text_engine.offset_at_point_attrs(
                        &v,
                        target_x,
                        cy - line_height,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
                };
                if new_cursor == cursor {
                    // No progress (already at the top/bottom edge): stop so a
                    // held arrow doesn't spin in place.
                    break;
                }
                self.cursor.set(new_cursor);
                steps -= 1;
            }
            self.last_edit.set(None);
            self.reveal_caret.set(true);
            self.push_cursor_to_source();
            self.push_selection_to_source();
        }

        // Caret position (focused only), computed once and reused for both the
        // scroll-to-caret adjustment and the caret draw below. Measured against
        // the full (wrapped) block via `caret_at_offset`, so a caret at a
        // soft-wrap boundary sits at the start of the next visual row rather
        // than the end of the previous one. While composing, it's measured
        // against the composed (preedit-spliced) string at `composed_caret`.
        let caret_xy = if self.focused {
            if let Some(ct) = composed_text.as_deref() {
                Some(ctx.text_engine.caret_at_offset_attrs(
                    ct,
                    composed_caret,
                    font_size,
                    line_height,
                    wrap_width,
                    &self.attrs,
                ))
            } else if value_is_empty {
                Some((0.0, 0.0))
            } else {
                let cursor = self.cursor.get();
                let v = self.value.borrow();
                Some(ctx.text_engine.caret_at_offset_attrs(
                    &v,
                    cursor,
                    font_size,
                    line_height,
                    wrap_width,
                    &self.attrs,
                ))
            }
        } else {
            None
        };

        // Scroll-to-caret: after an edit / navigation / click (which set the
        // reveal flag) nudge the viewport so the caret line is fully visible. A
        // wheel scroll does not set the flag, so mouse scrolling is not undone.
        if self.multiline {
            if self.reveal_caret.take() {
                if let Some((_cx, cy)) = caret_xy {
                    // Reveal against the logical target, then snap: a caret moved
                    // by typing / nav / click is shown immediately, never lazily
                    // glided to (only wheel input eases). A no-op reveal (caret
                    // already in range) leaves an in-flight glide untouched.
                    let mut t = self.scroll_anim.target();
                    if cy < t {
                        t = cy;
                    } else if cy + line_height > t + viewport_h {
                        t = cy + line_height - viewport_h;
                    }
                    t = t.clamp(0.0, max_scroll);
                    if t != self.scroll_anim.target() {
                        self.scroll_anim.snap(t);
                        displayed = t;
                    }
                }
            }
            self.last_max_scroll.set(max_scroll);
        }

        // Clip the text to the field's padding box and translate it up by the
        // scroll offset, so overflowing lines never cross the border or bleed
        // below the field. Single-line fields skip this (nothing to scroll or
        // clip vertically; horizontal overflow is intentional — see above).
        if self.multiline {
            ctx.push_clip(Rect::new(
                layout.origin.x,
                text_y,
                layout.size.width,
                viewport_h,
            ));
            ctx.push_offset(0.0, -displayed);
        }

        // Selection highlight, painted behind the glyphs so the (opaque) text
        // stays legible on top of the translucent fill. Suppressed while
        // composing — the composed string has its own (preedit) byte layout, so
        // a stale `value`-based selection range would land on the wrong glyphs.
        if self.focused && !composing {
            if let Some((lo, hi)) = self.selection_range() {
                let sel_color = self
                    .selection_color
                    .as_ref()
                    .map(|c| c.get())
                    .unwrap_or(ctx.theme.colors.selection_background);
                let rects = {
                    let v = self.value.borrow();
                    // `_with_trailing`: a multi-line selection draws a small
                    // sliver past each non-final row's last glyph so the
                    // included line breaks are visible (FW-6). Shaped with the
                    // field's attrs so the highlight tracks a bold value.
                    ctx.text_engine.selection_rects_with_trailing_attrs(
                        &v,
                        lo,
                        hi,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
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

        {
            let value = self.value.borrow();
            if value_is_empty {
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
            } else {
                // While composing, shape the preedit-spliced display string
                // plainly. The highlighter is intentionally bypassed mid-
                // composition (its color comes back the moment the IME commits)
                // — it tiles `value`, whose byte layout the preedit splice has
                // shifted, so reusing it here would miscolor. With a highlighter
                // and no composition, render through the rich path: tile the
                // buffer into color-only spans and shape them. Color-only spans
                // shape identically to the plain buffer (see
                // `build_highlight_spans`), so the caret math lines up with these
                // glyphs. Otherwise the plain path is used unchanged.
                let shaped = if let Some(ct) = composed_text.as_deref() {
                    ctx.text_engine.shape_text_attrs(
                        ct,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
                } else if let Some(hl) = self.highlighter.as_ref() {
                    let spans = build_highlight_spans(&value, hl(&value), &self.attrs);
                    ctx.text_engine
                        .shape_rich(&spans, font_size, line_height, wrap_width)
                } else {
                    ctx.text_engine.shape_text_attrs(
                        &value,
                        font_size,
                        line_height,
                        wrap_width,
                        &self.attrs,
                    )
                };

                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            text_x as i32 + glyph.x,
                            text_y as i32 + glyph.y,
                            image,
                            glyph.color.unwrap_or(text_color),
                            glyph.cache_key,
                        );
                    }
                }
            }
        }

        // IME composition underline: draw a thin rule under the preedit run so
        // the user can tell the uncommitted text apart from committed text (the
        // standard inline-composition affordance). Reuses `selection_rects` to
        // get one rect per visual line the preedit spans; the underline sits at
        // the bottom of each. Inside the multi-line clip/offset, so it scrolls
        // with the glyphs.
        if let (Some(ct), Some((ps, pe))) = (composed_text.as_deref(), preedit_span) {
            let rects = ctx.text_engine.selection_rects_attrs(
                ct,
                ps,
                pe,
                font_size,
                line_height,
                wrap_width,
                &self.attrs,
            );
            for r in rects {
                ctx.fill_rect(
                    Rect::new(
                        text_x + r.origin.x,
                        text_y + r.origin.y + r.size.height - 1.0,
                        r.size.width,
                        1.0,
                    ),
                    text_color,
                );
            }
        }

        // Caret (on top of the glyphs), shared by the empty and non-empty cases.
        // Anchors the IME candidate window so the OS composition UI follows the
        // caret instead of defaulting to a screen corner; the active offset folds
        // the scroll into the reported (window-relative) rect automatically.
        if let Some((cx, cy)) = caret_xy {
            let caret = Rect::new(text_x + cx, text_y + cy, 2.0, font_size);
            ctx.fill_rect(caret, text_color);
            // Report the cursor area to the OS. While composing, extend it down
            // past the preedit's underline (which sits at the line bottom,
            // `line_height` below the caret top) plus a small gap, so the OS
            // drops the candidate window *below* the underline instead of
            // landing its top edge on it. A `font_size`-tall box would end above
            // the underline and let the popup overlap it.
            let ime_h = if composing {
                line_height + 2.0
            } else {
                font_size
            };
            ctx.set_ime_cursor_area(Rect::new(caret.origin.x, caret.origin.y, 2.0, ime_h));
        }

        if self.multiline {
            ctx.pop_offset();
            ctx.pop_clip();
        }

        // Multi-line scrollbar indicator (overlay). Drawn after the text clip /
        // offset are popped, so it sits unclipped at viewport coords and never
        // scrolls with the glyphs. Mirrors `ScrollView::paint_post_children`.
        //
        // The bar rides the field's right edge, just inside the 1px border, in
        // the `SCROLLBAR_LANE` reserved out of `wrap_width` above. Because the
        // text wraps before that lane, no glyph or caret is drawn under the bar
        // (#34) — the lane is reserved unconditionally so the wrap width (and
        // thus the caret / hit-test / selection geometry keyed off it) doesn't
        // shift when the bar appears or disappears.
        if self.multiline && max_scroll > 0.0 && viewport_h > 0.0 {
            let content_h = viewport_h + max_scroll;
            let track_color = ctx.theme.colors.surface_variant;
            let thumb_color = ctx.theme.colors.on_surface_variant;

            let track_x = layout.right() - 1.0 - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
            let track_top = layout.origin.y + self.pad_y;
            ctx.fill_rect(
                Rect::new(track_x, track_top, SCROLLBAR_WIDTH, viewport_h),
                track_color,
            );

            // Thumb size proportional to the viewport/content ratio, floored so
            // it stays grabbable, capped at the track height.
            let thumb_h = ((viewport_h / content_h) * viewport_h)
                .max(SCROLLBAR_THUMB_MIN)
                .min(viewport_h);
            // Track the displayed (eased) offset so the thumb glides with the
            // text rather than jumping to the wheel target.
            let progress = (displayed / max_scroll).clamp(0.0, 1.0);
            let thumb_y = track_top + progress * (viewport_h - thumb_h);
            ctx.fill_rect(
                Rect::new(track_x, thumb_y, SCROLLBAR_WIDTH, thumb_h),
                thumb_color,
            );
        }

        if focus_active && !border_focus {
            // Ring tracks the field's corner radius, so a rounded input gets a
            // rounded ring instead of a square one around rounded corners.
            // Skipped when `Border` mode already recolored the border above.
            ctx.paint_focus_ring(
                layout,
                self.focus_ring_color.as_ref().map(|c| c.get()),
                self.radius,
            );
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
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
                    // Chain quick presses at the same spot into a multi-click:
                    // 1 = caret, 2 = word, 3 = whole line, then cycle back to 1.
                    // The hit-test and the word / line expansion are resolved at
                    // paint (no text engine here).
                    let now = Instant::now();
                    let count = match self.last_click.get() {
                        Some((t, p, c))
                            if now.duration_since(t) <= DOUBLE_CLICK_MAX
                                && (p.x - position.x).abs() <= DOUBLE_CLICK_SLOP
                                && (p.y - position.y).abs() <= DOUBLE_CLICK_SLOP =>
                        {
                            c % 3 + 1
                        }
                        _ => 1,
                    };
                    self.last_click.set(Some((now, *position, count)));
                    let unit = match count {
                        2 => SelectUnit::Word,
                        3 => SelectUnit::Line,
                        _ => SelectUnit::Caret,
                    };
                    self.pending_hit.set(Some((*position, ctx.modifiers.shift)));
                    self.pending_select.set(unit);
                    self.selecting.set(true);
                    self.desired_x.set(None);
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

            // Mouse wheel over a multi-line field scrolls its internal viewport.
            // Only consumed when there is actually content to scroll (so a
            // non-overflowing field lets the wheel bubble to an outer scroller);
            // `last_max_scroll` is the bound computed by the last paint. Paint
            // re-clamps authoritatively, so the raw set here is safe.
            WidgetEvent::Scroll {
                position, delta_y, ..
            } if self.multiline => {
                let max = self.last_max_scroll.get();
                if !layout.contains(*position) || max <= 0.0 {
                    EventResult::Ignored
                } else {
                    // Accumulate against the target so consecutive ticks add up;
                    // `set` eases the displayed offset toward it (paint re-clamps
                    // authoritatively, so this raw bound is safe). Guarded so a
                    // tick at a scroll bound doesn't restart the glide.
                    let new_y = (self.scroll_anim.target() - delta_y).clamp(0.0, max);
                    if new_y != self.scroll_anim.target() {
                        self.scroll_anim.set(new_y);
                    }
                    EventResult::Consumed
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
                // Drop any half-composed IME preedit so it can't linger on
                // screen after the field is no longer focused.
                self.preedit.clear();
                self.preedit_cursor = None;
                self.pending_select.set(SelectUnit::Caret);
                self.desired_x.set(None);
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
                    self.desired_x.set(None);
                    // `had_selection` only decided whether this insert *starts*
                    // a fresh step (above); typing that follows still coalesces
                    // into it, so the run isn't broken here.
                    self.commit_edit(EditKind::Insert, false, ctx);
                }
                EventResult::Consumed
            }

            WidgetEvent::ImePreedit { text, cursor } if self.focused => {
                // Store the in-progress composition for display only — `value`
                // is untouched until the IME commits (which arrives via
                // CharInput). An empty `text` clears the preedit (commit /
                // cancel). Reveal the caret so a composition that grows past
                // the viewport stays visible.
                self.preedit.clear();
                self.preedit.push_str(text);
                self.preedit_cursor = *cursor;
                self.reveal_caret.set(true);
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
                            // Smart keymap (B-1 ③): a hook may delete a whole
                            // structural prefix (e.g. a markdown list marker) as
                            // one step. `None` (or a malformed edit) falls
                            // through to the single-char delete, which coalesces
                            // with a run of Backspaces.
                            let smart = self.backspace_handler.as_ref().and_then(|h| {
                                let v = self.value.borrow();
                                h(&v, cursor)
                            });
                            let handled = match smart {
                                Some(edit) => self.apply_key_edit(EditKind::Delete, edit, ctx),
                                None => false,
                            };
                            if !handled {
                                let prev = {
                                    let v = self.value.borrow();
                                    Self::prev_char_boundary(&v, cursor)
                                };
                                self.begin_edit(EditKind::Delete, true);
                                self.value.borrow_mut().drain(prev..cursor);
                                self.cursor.set(prev);
                                self.desired_x.set(None);
                                self.commit_edit(EditKind::Delete, false, ctx);
                            }
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
                            self.desired_x.set(None);
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
                    self.desired_x.set(None);
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
                    self.desired_x.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowUp) if self.multiline => {
                    // Shift extends the selection; a plain vertical move drops
                    // it. The caret relocation itself needs the text engine to
                    // map to/from visual rows, so it's deferred to paint via
                    // `pending_vmove` (like a click's `pending_hit`); `desired_x`
                    // is left untouched here so a run of Up/Down keeps its
                    // sticky column.
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
                    self.pending_vmove.set(self.pending_vmove.get() - 1);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::ArrowDown) if self.multiline => {
                    if ctx.modifiers.shift {
                        self.ensure_anchor();
                    } else {
                        self.clear_selection();
                    }
                    self.pending_vmove.set(self.pending_vmove.get() + 1);
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
                    self.desired_x.set(None);
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
                    self.desired_x.set(None);
                    self.last_edit.set(None);
                    EventResult::Consumed
                }
                Key::Named(NamedKey::Enter) => {
                    if self.multiline {
                        // Smart keymap (B-1 ③): with no active selection, let an
                        // app-supplied hook turn Enter into a structural edit —
                        // continuing a markdown list, or clearing an empty list
                        // item — instead of a plain newline. `None` (or a
                        // malformed edit) falls through. A selection always falls
                        // through to the newline insert below.
                        let smart = if self.selection_range().is_none() {
                            self.enter_handler.as_ref().and_then(|h| {
                                let v = self.value.borrow();
                                h(&v, self.cursor.get())
                            })
                        } else {
                            None
                        };
                        let handled = match smart {
                            Some(edit) => self.apply_key_edit(EditKind::Insert, edit, ctx),
                            None => false,
                        };
                        if !handled {
                            // Insert a newline at the cursor; on_submit is
                            // intentionally inert in multi-line mode so the field
                            // behaves like a textarea. A newline is its own
                            // discrete undo step — it doesn't merge with the
                            // typing on either side.
                            let cursor = self.cursor.get();
                            self.begin_edit(EditKind::Insert, false);
                            self.value.borrow_mut().insert(cursor, '\n');
                            self.cursor.set(cursor + 1);
                            self.desired_x.set(None);
                            self.commit_edit(EditKind::Insert, true, ctx);
                        }
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
        // Any consumed edit / caret move should bring the caret back into view
        // on the next paint. A wheel scroll is the one consumed event that must
        // *not* (so scrolling away from the caret with the mouse sticks).
        if self.multiline
            && matches!(result, EventResult::Consumed)
            && !matches!(event, WidgetEvent::Scroll { .. })
        {
            self.reveal_caret.set(true);
        }
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
    use super::{line_bounds, word_bounds};

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
        // Each kana is 3 UTF-8 bytes; the run is all one script (hiragana) so
        // all three are selected, and the bounds land on char starts (0 and 9).
        assert_eq!(word_bounds("あいう", 3), (0, 9));
    }

    #[test]
    fn word_bounds_segments_by_script() {
        // Japanese has no inter-word spaces, so folding every CJK char into one
        // `Word` run made a double-click grab the whole sentence. Splitting on
        // script runs (FW-11) stops at a script change: a click in the kanji
        // grabs only the kanji, a click in the trailing kana grabs the kana.
        // "日本語です" = 日本語 (3 kanji) + です (2 hiragana), 3 bytes each.
        let s = "日本語です";
        assert_eq!(word_bounds(s, 3), (0, 9)); // inside the kanji run
        assert_eq!(word_bounds(s, 12), (9, 15)); // inside the trailing hiragana
    }

    #[test]
    fn word_bounds_keeps_katakana_run_with_choonpu() {
        // The prolonged sound mark `ー` (U+30FC) is katakana, not a generic
        // word char, so "ラーメン" selects as one run instead of splitting at ー.
        assert_eq!(word_bounds("ラーメン", 6), (0, 12));
    }

    #[test]
    fn word_bounds_splits_hiragana_from_katakana() {
        // Distinct kana scripts are distinct runs: a click in the katakana
        // grabs only the katakana. "あアい" → あ(0..3) ア(3..6) い(6..9).
        assert_eq!(word_bounds("あアい", 3), (3, 6));
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

    #[test]
    fn line_bounds_grabs_middle_line_excluding_breaks() {
        // Triple-click anywhere on "bar" selects "bar" — no surrounding `\n`.
        let s = "foo\nbar\nbaz";
        assert_eq!(line_bounds(s, 4), (4, 7)); // start of "bar"
        assert_eq!(line_bounds(s, 6), (4, 7)); // inside "bar"
        assert_eq!(line_bounds(s, 7), (4, 7)); // end of "bar", on the `\n`
    }

    #[test]
    fn line_bounds_first_and_last_line() {
        let s = "foo\nbar\nbaz";
        assert_eq!(line_bounds(s, 1), (0, 3)); // first line
        assert_eq!(line_bounds(s, 9), (8, 11)); // last line
        assert_eq!(line_bounds(s, 11), (8, 11)); // end of text
    }

    #[test]
    fn line_bounds_empty_line_is_caret() {
        // A blank line between two breaks collapses to a zero-width range at it.
        let s = "foo\n\nbar";
        assert_eq!(line_bounds(s, 4), (4, 4));
    }

    #[test]
    fn line_bounds_single_line_is_whole_buffer() {
        assert_eq!(line_bounds("hello world", 5), (0, 11));
        assert_eq!(line_bounds("", 0), (0, 0));
    }

    #[test]
    fn line_bounds_respects_char_boundaries() {
        // Multibyte content: bounds land on `\n` / buffer edges (char starts).
        let s = "あい\nうえ"; // あい = bytes 0..6, \n = 6, うえ = 7..13
        assert_eq!(line_bounds(s, 3), (0, 6)); // inside first line
        assert_eq!(line_bounds(s, 10), (7, 13)); // inside second line
    }
}
