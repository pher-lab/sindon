//! Core text engine: shaping + rasterization.

use crate::attrs::{FontStyle, TextAttrs};
use crate::span::{TextSpan, cosmic_to_shroud, shroud_to_cosmic};
use cosmic_text::{
    Attrs, AttrsList, BidiParagraphs, Buffer, BufferLine, Cursor, FontSystem, LineEnding, LineIter,
    Metrics, Scroll, Shaping, SwashCache, SwashContent,
};
use shroud_core::{Color, Rect};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A positioned glyph ready for rasterization.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Cache key for rasterization lookup.
    pub cache_key: cosmic_text::CacheKey,
    /// Logical-pixel X position, relative to the shaped run's origin.
    ///
    /// Fractional by construction on a HiDPI display: placement is computed on
    /// the *physical* pixel grid (that snapping is what keeps glyphs crisp) and
    /// then divided back into logical space, so a glyph can legitimately land on
    /// a half-pixel. Callers add their logical origin and the renderer
    /// multiplies the scale back in, which returns the glyph to the exact device
    /// pixel it was rasterized for.
    pub x: f32,
    /// Logical-pixel Y position (the baseline). See [`x`](Self::x).
    pub y: f32,
    /// Per-glyph color override. `Some` only when the glyph came from a
    /// `TextSpan` with a color set in [`TextEngine::shape_rich`]; the
    /// single-attrs `shape_text` / `shape_text_attrs` paths always emit `None`
    /// (the renderer falls back to the widget-level color in that case).
    pub color: Option<Color>,
}

/// Block-relative bounding box of all glyphs on one visual line that share a
/// single source span (the span's index in the slice passed to
/// [`TextEngine::shape_rich`]).
///
/// A span that wraps across N visual lines produces N `SpanBox`es — one per
/// line — so a multi-line link's hit region tracks each line's run. The
/// `rect` is in the same block-relative coordinate space the widget paints
/// glyphs into (origin at the shaped block's top-left), so a caller adds the
/// widget's layout origin to translate into screen space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanBox {
    /// Index of the source span in the `shape_rich` input slice.
    pub span: usize,
    /// Bounding rectangle of this span's glyphs on one line.
    pub rect: Rect,
}

/// A line decoration (underline or strike-through) to fill under/over a span's
/// glyphs. cosmic-text shapes glyphs but does not draw decorations, so
/// [`shape_rich`](TextEngine::shape_rich) emits these geometrically from the
/// baseline and the renderer draws them as thin filled rectangles.
///
/// `rect` is block-relative (origin at the shaped block's top-left), the same
/// space as [`SpanBox`] and the painted glyph positions, so a caller adds the
/// widget's layout origin to translate into screen space. `color` mirrors
/// [`ShapedGlyph::color`]: `Some` when the source span set an explicit color,
/// `None` to fall back to the widget-level color (so the decoration always
/// matches the text it decorates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecorationLine {
    /// Filled rectangle for the decoration, block-relative.
    pub rect: Rect,
    /// Span color override, or `None` to use the widget-level color.
    pub color: Option<Color>,
}

/// Result of shaping a text string.
///
/// `Clone` so the shape cache can hand back an owned copy on a hit without
/// re-borrowing the engine: callers iterate the glyphs while also calling
/// `rasterize(&mut self)`, so a borrowed return would conflict. The clone is a
/// few `Vec` copies (glyph keys + positions) — far cheaper than re-shaping.
#[derive(Debug, Clone)]
pub struct ShapedText {
    /// Positioned glyphs in draw order.
    pub glyphs: Vec<ShapedGlyph>,
    /// Total width in pixels (from layout).
    pub width: f32,
    /// Total height in pixels (from layout).
    pub height: f32,
    /// Per-span, per-line bounding boxes. Only populated by
    /// [`shape_rich`](TextEngine::shape_rich) — the single-attrs
    /// `shape_text` / `shape_text_attrs` paths leave this empty since they
    /// have no span structure. Used by `TextWidget` to map a click position
    /// back to the span that owns it (and thus its link target).
    pub span_boxes: Vec<SpanBox>,
    /// Underline / strike-through rectangles, one per decorated span per
    /// visual line. Only populated by [`shape_rich`](TextEngine::shape_rich)
    /// (decoration is a `TextSpan` property); the single-attrs paths leave it
    /// empty. The widget fills each as a rectangle in its text color.
    pub decoration_lines: Vec<DecorationLine>,
}

/// Everything the IME-composition paint path needs from a single shaping of the
/// preedit-spliced display string, produced by
/// [`shape_composing`](TextEngine::shape_composing).
///
/// A composing `Input` used to shape the whole (potentially long) note three
/// times per preedit update — once for glyphs, once for the caret, once for the
/// preedit underline — and every keystroke produces a unique string that never
/// hits the shape cache, so the cost grew with note length. Deriving all three
/// from one buffer collapses that to a single shape.
#[derive(Debug, Clone)]
pub struct ComposedBlock {
    /// Positioned glyphs and the block's total width/height.
    pub shaped: ShapedText,
    /// Block-relative caret position at the requested offset, matching
    /// [`caret_at_offset_attrs`](TextEngine::caret_at_offset_attrs).
    pub caret: (f32, f32),
    /// Block-relative underline rects for the preedit run, one per visual line
    /// it spans, matching [`selection_rects_attrs`](TextEngine::selection_rects_attrs).
    pub underline: Vec<Rect>,
    /// Block-relative highlight rects for the IME's *target clause* — the
    /// 注目文節 winit reports as a byte range `[cs, ce)` with `cs != ce` while
    /// `space` cycles conversion candidates. One rect per visual row it spans,
    /// full line-height like a selection so the converting clause reads as
    /// highlighted. Empty while merely typing (the IME reports a caret, not a
    /// range) — the plain caret covers that case.
    pub target: Vec<Rect>,
}

/// A shaped, queryable buffer for the focused (non-composing) editing path,
/// produced by [`shape_edit_plain`](TextEngine::shape_edit_plain) /
/// [`shape_edit_rich`](TextEngine::shape_edit_rich).
///
/// The focused paint used to shape the whole (potentially long) value up to
/// three times per keystroke: the content height misses the cache on every
/// edit, the glyphs miss it *separately* under a highlighter (their rich key
/// differs from the plain one), and the caret never consulted the cache at
/// all. Like [`ComposedBlock`] did for the IME path, deriving the content
/// height, glyphs, caret, selection rects, and click hit-tests from one buffer
/// collapses that to a single shape per frame.
///
/// Unlike `ComposedBlock`, the derived queries are methods rather than
/// precomputed fields: the caret and selection move *during* paint (deferred
/// click / arrow-move resolution), so they must be readable after the buffer
/// is shaped.
///
/// Holds the glyphs behind an `Rc` because a repaint whose inputs are
/// unchanged hands back the *same* `ShapedText` the previous frame produced.
/// Cloning it instead would reintroduce exactly the cost the `measure_*` split
/// was added to remove.
///
/// The cosmic buffer itself lives on the engine (`edit_slot`), not here, so it
/// can be **reused across frames** — that is what makes an edit re-shape only
/// the lines it touched. This value is therefore just the frame's glyphs plus
/// a witness that the slot is populated; the derived queries
/// ([`edit_hit`](TextEngine::edit_hit),
/// [`edit_selection_rects_with_trailing`](TextEngine::edit_selection_rects_with_trailing),
/// [`edit_caret`](TextEngine::edit_caret)) read the slot and are only valid in
/// the same frame that shaped it.
pub struct EditBuffer {
    shaped: std::rc::Rc<ShapedText>,
}

impl EditBuffer {
    /// Positioned glyphs and the block's total width/height, matching what
    /// [`shape_text_attrs`](TextEngine::shape_text_attrs) (plain) or
    /// [`shape_rich`](TextEngine::shape_rich) (rich) returns for the same
    /// inputs.
    pub fn shaped(&self) -> &ShapedText {
        &self.shaped
    }
}

/// A rasterized glyph image.
///
/// Most glyphs are single-channel **alpha masks** (`is_color == false`): one
/// byte per pixel, tinted by the text color at draw time. Color emoji
/// (COLR / embedded bitmap faces) come back from swash as full **RGBA**
/// (`is_color == true`): four bytes per pixel carrying their own colors,
/// which must *not* be recolored by the text color. The renderer routes the
/// two kinds to different atlases (R8 vs RGBA8) based on this flag.
pub struct GlyphImage {
    /// Pixel data. One byte per pixel (alpha) when `is_color` is false; four
    /// bytes per pixel (RGBA, straight alpha) when `is_color` is true.
    pub data: Vec<u8>,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// X bearing (offset from glyph origin to left edge of bitmap).
    pub left: i32,
    /// Y bearing (offset from glyph origin to top edge of bitmap).
    pub top: i32,
    /// `true` when `data` is RGBA color (a color emoji); `false` for an
    /// alpha mask.
    pub is_color: bool,
}

impl std::fmt::Debug for GlyphImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("left", &self.left)
            .field("top", &self.top)
            .field("is_color", &self.is_color)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Max number of distinct shaping results held by [`TextEngine`]'s cache.
///
/// One entry per `(text, attrs, metrics, wrap-width)` tuple. The hot case — a
/// markdown preview repainted every idle tick — touches its whole paragraph
/// set each frame, so the cache only helps when that set fits; this covers
/// notes up to several hundred paragraphs (each shaped at its natural and
/// wrapped widths). Past the cap, least-recently-used entries are evicted and
/// re-shaped on demand, degrading gracefully rather than growing unbounded.
const SHAPE_CACHE_CAP: usize = 1024;

/// Bounded, least-recently-used cache of shaping results.
///
/// Keyed by a 64-bit digest of the shaping inputs (text + attrs + metrics +
/// wrap width), never the source string — so the cache stores no plaintext in
/// its keys. The cached [`ShapedText`] holds glyph cache-keys and pixel
/// positions (not the text), and the whole cache is dropped on a screen swap
/// (e.g. a vault lock) so nothing note-derived outlives the screen that made
/// it. Secret reveals (`SecureText`) bypass the cache entirely via the
/// `*_uncached` shaping entry points.
struct ShapeCache {
    /// digest -> (result, last-touched logical clock).
    map: HashMap<u64, (ShapedText, u64)>,
    clock: u64,
}

impl ShapeCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            clock: 0,
        }
    }

    /// Return an owned copy of the cached result for `key`, bumping its
    /// recency, or `None` on a miss.
    fn get(&mut self, key: u64) -> Option<ShapedText> {
        self.clock += 1;
        let clock = self.clock;
        self.map.get_mut(&key).map(|entry| {
            entry.1 = clock;
            entry.0.clone()
        })
    }

    /// Return just the cached `(width, height)` for `key`, bumping recency,
    /// without copying the glyph vector.
    ///
    /// The layout pass only ever asks text how big it is, and it asks several
    /// times per widget per frame (Taffy probes min-content and max-content
    /// before the final pass). Serving those through [`get`](Self::get) meant
    /// a full `Vec<ShapedGlyph>` clone every time — measured at ~5.2 ns per
    /// glyph, which is ~90% of the layout cost of a text-heavy tree.
    fn dimensions(&mut self, key: u64) -> Option<(f32, f32)> {
        self.clock += 1;
        let clock = self.clock;
        self.map.get_mut(&key).map(|entry| {
            entry.1 = clock;
            (entry.0.width, entry.0.height)
        })
    }

    /// Insert `value` under `key`, evicting the least-recently-used entry first
    /// if a *new* key would push the map past the cap.
    fn insert(&mut self, key: u64, value: ShapedText) {
        self.clock += 1;
        let clock = self.clock;
        if self.map.len() >= SHAPE_CACHE_CAP && !self.map.contains_key(&key) {
            if let Some((&lru, _)) = self.map.iter().min_by_key(|(_, (_, used))| *used) {
                self.map.remove(&lru);
            }
        }
        self.map.insert(key, (value, clock));
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

/// Fold the per-span style inputs that change the shaped output into `h`.
/// `link` is deliberately excluded — it never affects glyphs or boxes (the
/// widget maps a span index back to its link from the live span list, not the
/// cache).
fn hash_attrs(attrs: &TextAttrs, h: &mut DefaultHasher) {
    attrs.family.hash(h);
    attrs.weight.0.hash(h);
    // `FontStyle` (cosmic-text's `Style`) has no `Hash`; map to a discriminant.
    let style_tag: u8 = match attrs.style {
        FontStyle::Normal => 0,
        FontStyle::Italic => 1,
        FontStyle::Oblique => 2,
    };
    style_tag.hash(h);
}

fn hash_metrics(
    font_size: f32,
    line_height: f32,
    max_width: Option<f32>,
    scale: f32,
    h: &mut DefaultHasher,
) {
    font_size.to_bits().hash(h);
    line_height.to_bits().hash(h);
    // The shaped output carries glyph cache keys stamped with `font_size *
    // scale`, so an entry shaped for one display is the wrong bitmap on
    // another. Layout would be identical — only the rasterization differs —
    // which is exactly the kind of miss that survives a review: the text is in
    // the right place, just soft.
    scale.to_bits().hash(h);
    match max_width {
        Some(w) => {
            1u8.hash(h);
            w.to_bits().hash(h);
        }
        None => 0u8.hash(h),
    }
}

/// Digest for a single-attrs (plain) shaping call. Domain-tagged `0` so it can
/// never collide with a rich digest of the same text.
fn shape_key_plain(
    text: &str,
    font_size: f32,
    line_height: f32,
    max_width: Option<f32>,
    attrs: &TextAttrs,
    scale: f32,
) -> u64 {
    let mut h = DefaultHasher::new();
    0u8.hash(&mut h);
    text.hash(&mut h);
    hash_attrs(attrs, &mut h);
    hash_metrics(font_size, line_height, max_width, scale, &mut h);
    h.finish()
}

/// Digest for a composing (IME preedit) shaping call. Domain-tagged `2`, so it
/// can never collide with a plain (`0`) or rich (`1`) key — which is what lets
/// all three share `slot_key` without a composing digest ever being mistaken
/// for "the slot holds this editable value".
///
/// Folds the plain inputs plus the three derived-geometry arguments, because a
/// [`ComposedBlock`] carries its caret and rects as values: moving the caret
/// inside an unchanged preedit changes the answer without changing the text.
#[allow(clippy::too_many_arguments)]
fn shape_key_composing(
    text: &str,
    font_size: f32,
    line_height: f32,
    max_width: Option<f32>,
    attrs: &TextAttrs,
    scale: f32,
    caret_offset: usize,
    underline_range: (usize, usize),
    target_range: Option<(usize, usize)>,
) -> u64 {
    let mut h = DefaultHasher::new();
    2u8.hash(&mut h);
    text.hash(&mut h);
    hash_attrs(attrs, &mut h);
    hash_metrics(font_size, line_height, max_width, scale, &mut h);
    caret_offset.hash(&mut h);
    underline_range.hash(&mut h);
    target_range.hash(&mut h);
    h.finish()
}

/// Digest for a rich (multi-span) shaping call. Domain-tagged `1`. Folds every
/// input that moves a glyph or a decoration: each span's text, attrs, color,
/// and decoration flags, plus the shared metrics.
fn shape_key_rich(
    spans: &[TextSpan],
    font_size: f32,
    line_height: f32,
    max_width: Option<f32>,
    scale: f32,
) -> u64 {
    let mut h = DefaultHasher::new();
    1u8.hash(&mut h);
    spans.len().hash(&mut h);
    for s in spans {
        s.text.hash(&mut h);
        hash_attrs(&s.attrs, &mut h);
        match s.color {
            Some(c) => {
                1u8.hash(&mut h);
                c.r.to_bits().hash(&mut h);
                c.g.to_bits().hash(&mut h);
                c.b.to_bits().hash(&mut h);
                c.a.to_bits().hash(&mut h);
            }
            None => 0u8.hash(&mut h),
        }
        s.decoration.underline.hash(&mut h);
        s.decoration.strikethrough.hash(&mut h);
    }
    hash_metrics(font_size, line_height, max_width, scale, &mut h);
    h.finish()
}

/// Text engine: owns the font system and glyph cache.
///
/// Create one per application. Not Send — cosmic-text's FontSystem is not
/// thread-safe.
pub struct TextEngine {
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: ShapeCache,
    /// Physical pixels per logical pixel, pushed by the app each frame.
    ///
    /// Shaping itself stays in logical units — metrics, wrap width, caret
    /// geometry and the reported width/height are all scale-free, so a DPI
    /// change cannot move a caret or re-wrap a paragraph. The factor is used
    /// only when placing glyphs: `LayoutGlyph::physical` snaps each one to the
    /// physical pixel grid and stamps the scaled size into its cache key, so
    /// the rasterizer produces a bitmap at device resolution instead of a
    /// logical-size one stretched to fit.
    scale: f32,
    /// Full buffer shapes performed since the last
    /// [`take_shape_stats`](Self::take_shape_stats) drain, and the time spent
    /// in them. Always-on (two counter bumps per shape); read per frame by the
    /// app layer when perf logging is enabled.
    shape_count: u32,
    shape_ns: u64,
    /// Lines the incremental editing path had to re-shape since the last
    /// [`take_reshaped_lines`](Self::take_reshaped_lines) drain. This is the
    /// number that says whether the reuse is working: one keystroke inside a
    /// long note should report `1`, not the document's line count.
    reshaped_lines: u32,
    /// The cosmic buffer behind the focused editing path, kept alive **across
    /// frames** so an edit re-shapes only the lines it changed.
    ///
    /// cosmic-text already caches shaping and layout per `BufferLine`, but
    /// `Buffer::set_text` clears the whole line vector (and `set_rich_text`
    /// resets every line unconditionally), so building a fresh buffer each
    /// frame threw that cache away — which is why typing one character into a
    /// long note re-shaped the entire document. Holding one buffer and
    /// rewriting only the differing lines (`sync_edit_lines`) keeps the rest of
    /// the document's shape and layout.
    ///
    /// **Lifetime contract:** the lines hold the field's plaintext, so the slot
    /// must be dropped everywhere the shape cache is
    /// ([`clear_shape_cache`](Self::clear_shape_cache) — screen swap / lock).
    /// The vendored cosmic-text zeroizes `BufferLine.text` on overwrite and on
    /// drop, so dropping the slot wipes it; the Phase 43 residue gate is what
    /// holds this honest. One slot is enough because exactly one field is
    /// focused at a time, and a slot left over from a *different* field is only
    /// a missed optimization, never a wrong answer: the line sync makes the
    /// buffer's content equal the requested text either way.
    ///
    /// Three paths write it — [`shape_edit_plain`](Self::shape_edit_plain),
    /// [`shape_edit_rich`](Self::shape_edit_rich) and
    /// [`shape_composing`](Self::shape_composing) — because a field alternates
    /// between editing and composing and the two strings differ by only the
    /// preedit, so sharing one buffer makes the commit transition nearly free.
    /// Each writer must record what it left behind in `slot_key`.
    edit_slot: Option<Buffer>,
    /// Digest of the text `edit_slot` currently holds, set by every path that
    /// syncs it.
    ///
    /// The buffer is what `edit_caret` / `edit_hit` /
    /// `edit_selection_rects_with_trailing` read, so serving a memo whose
    /// glyphs describe a *different* string than the slot holds would pair
    /// correct glyphs with a geometry source answering about something else.
    /// That is not hypothetical: cancelling an IME composition leaves `value`
    /// byte-identical to what it was before composing started, so the edit
    /// memo's key matches while the slot still holds the composed string.
    /// Gating the memo on this — rather than on the slot merely being
    /// non-empty — makes the pairing structural, so a fourth writer of the slot
    /// inherits the guarantee by setting this field.
    slot_key: Option<u64>,
    /// Digest of the inputs `edit_slot` was last shaped for, paired with the
    /// glyphs that came out.
    ///
    /// Line reuse made a keystroke cheap, but it still walked the whole
    /// document every frame to *discover* what changed — and in a real editing
    /// session only ~9% of frames change anything, so ~91% were paying that
    /// walk for nothing (measured on knot at 0.78 ms mean, 3.0 ms worst, per
    /// idle frame). Comparing one digest first skips the walk outright.
    ///
    /// Keyed by the same `shape_key_*` digest the shape cache uses, so — like
    /// that cache — it stores **no plaintext**, only a hash and the resulting
    /// glyph positions. Dropped alongside the slot in
    /// [`clear_shape_cache`](Self::clear_shape_cache).
    edit_memo: Option<(u64, std::rc::Rc<ShapedText>)>,
    /// The composing twin of `edit_memo`: the last [`ComposedBlock`] and the
    /// digest of everything it was derived from.
    ///
    /// Composition is the one input method that repaints without changing
    /// anything — the candidate window opening, a hover, the IME watchdog's
    /// forced redraw — and each of those used to rebuild the whole document.
    ///
    /// Unlike `edit_memo` this deliberately does **not** require the slot to
    /// still describe the same string. A `ComposedBlock` is self-contained: its
    /// caret, underline and target rects are precomputed fields, not queries
    /// against the buffer, so it stays correct however the slot has moved on.
    /// Everything it depends on is in the key.
    compose_memo: Option<(u64, std::rc::Rc<ComposedBlock>)>,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// Create a new text engine with system fonts.
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            shape_cache: ShapeCache::new(),
            scale: 1.0,
            shape_count: 0,
            shape_ns: 0,
            reshaped_lines: 0,
            edit_slot: None,
            slot_key: None,
            edit_memo: None,
            compose_memo: None,
        }
    }

    /// Physical pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Set the scale glyphs rasterize at. The app pushes the window's current
    /// value each frame.
    ///
    /// Shaped output is cached under this factor (glyph cache keys carry the
    /// scaled size), so changing it selects fresh entries rather than reusing
    /// bitmaps rasterized for the old display.
    pub fn set_scale(&mut self, scale: f32) {
        if scale > 0.0 {
            self.scale = scale;
        }
    }

    /// Drain the full-shape counters accumulated since the last call:
    /// `(count, nanoseconds)` across every cosmic buffer build — cache misses,
    /// the composing / editing single-shapes, and standalone caret / hit-test
    /// shapes alike. Cache *hits* don't count (nothing was shaped). This is
    /// the measurement hook behind the frame perf log (`SHROUD_PERF`).
    pub fn take_shape_stats(&mut self) -> (u32, u64) {
        let stats = (self.shape_count, self.shape_ns);
        self.shape_count = 0;
        self.shape_ns = 0;
        stats
    }

    /// Drop every cached shaping result.
    ///
    /// Called on a screen swap (the framework clears it when the tree root is
    /// replaced) so glyph geometry derived from one screen's text — which for a
    /// notes app is the user's plaintext — does not outlive that screen. Also
    /// the escape hatch if a font is registered at runtime and previously-shaped
    /// text should be re-evaluated against it.
    ///
    /// Drops the persistent editing buffer (`edit_slot`) for the same reason:
    /// its `BufferLine`s hold the focused field's plaintext, and dropping them
    /// zeroizes it.
    pub fn clear_shape_cache(&mut self) {
        self.shape_cache.clear();
        self.edit_slot = None;
        self.slot_key = None;
        self.edit_memo = None;
        self.compose_memo = None;
    }

    /// Access the underlying FontSystem (for advanced usage).
    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Register a font from in-memory bytes (TTF / OTF) so its families become
    /// resolvable by name in [`TextAttrs`](crate::TextAttrs)/`TextFamily::Named`.
    ///
    /// Returns the family names that the loaded faces expose, de-duplicated and
    /// in first-seen order, so the caller can build a `TextFamily::Named(..)`
    /// without hard-coding the font's internal family string. An empty `Vec`
    /// means the bytes held no parseable face (bad data) — nothing was added.
    ///
    /// This is the first-class entry point for bundling an *icon font*: load a
    /// monochrome icon `.ttf` once at startup (see `App::font`), then draw an
    /// icon as a single-glyph `TextWidget` in that family — the glyph flows
    /// through the same shaping / atlas / tint path as text, so it scales and
    /// recolors for free (a color/COLR face routes through the color atlas like
    /// emoji instead). Icons are not secret, so loading one is unrelated to the
    /// zeroize path; the shape cache stays coherent because we drop it here, so
    /// any text shaped before this call is re-evaluated against the new font on
    /// its next paint. Call before the first paint for startup fonts.
    pub fn load_font_data(&mut self, data: &[u8]) -> Vec<String> {
        use std::collections::HashSet;

        let before: HashSet<cosmic_text::fontdb::ID> =
            self.font_system.db().faces().map(|f| f.id).collect();
        self.font_system.db_mut().load_font_data(data.to_vec());

        let mut names = Vec::new();
        for face in self.font_system.db().faces() {
            if before.contains(&face.id) {
                continue;
            }
            if let Some((family, _)) = face.families.first() {
                if !names.contains(family) {
                    names.push(family.clone());
                }
            }
        }

        // A newly registered font can change how already-cached text resolves
        // (e.g. a `Named` family that previously fell back). Drop the cache so
        // the next paint re-shapes against the current font set. At startup the
        // cache is empty, so this is free in the common case.
        self.clear_shape_cache();
        names
    }

    /// Redefine the concrete font family that generic / unstyled text resolves
    /// to.
    ///
    /// A widget that never calls `.family(..)` shapes as
    /// [`TextFamily::SansSerif`](crate::TextFamily), which cosmic-text resolves
    /// through fontdb's *sans-serif* generic. cosmic defaults that generic to
    /// `"Open Sans"` — a family absent from a stock Windows install — so Latin
    /// text lands on whatever fontdb substitutes while CJK text is served by a
    /// *separate* per-script fallback. The two typefaces disagree on x-height,
    /// weight, and metrics, which reads as the ragged "text forced through an
    /// editor that doesn't do Japanese" look.
    ///
    /// Point the sans-serif generic at one family that covers every script the
    /// app renders — a bundled `"Noto Sans JP"` (register its bytes first via
    /// [`load_font_data`](Self::load_font_data)) or an installed `"Yu Gothic
    /// UI"` — and unstyled text shapes in that single typeface end to end, with
    /// no cross-script mixing. Explicit `Monospace` / `Named` families (code
    /// spans, the icon font) are unaffected; only the default is remapped.
    ///
    /// A `name` fontdb can't resolve degrades to the prior behavior rather than
    /// erroring, so this is safe to call speculatively. Drops the shape cache so
    /// text shaped before the swap re-resolves against the new default.
    pub fn set_default_font_family(&mut self, name: &str) {
        self.font_system.db_mut().set_sans_serif_family(name);
        self.clear_shape_cache();
    }

    /// Shape a text string with default attributes (sans-serif, normal weight,
    /// normal style). Equivalent to calling [`shape_text_attrs`](Self::shape_text_attrs)
    /// with `TextAttrs::default()`; retained as the original API used by
    /// widgets that don't need font customization.
    pub fn shape_text(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        self.shape_text_attrs(
            text,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Shape a text string into positioned glyphs with the given font
    /// attributes.
    ///
    /// `font_size`: font size in pixels.
    /// `line_height`: line height in pixels.
    /// `max_width`: optional max width for line wrapping.
    /// `attrs`: family / weight / style. Default attrs match cosmic-text's
    /// `Attrs::new()`.
    ///
    /// Cached: identical inputs return a cloned previous result instead of
    /// re-shaping. This is what keeps a markdown preview cheap to repaint every
    /// idle tick — unchanged paragraphs become an `O(1)` lookup. Secret reveals
    /// must use [`shape_text_uncached`](Self::shape_text_uncached) so they never
    /// land in the cache.
    pub fn shape_text_attrs(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> ShapedText {
        let key = shape_key_plain(text, font_size, line_height, max_width, attrs, self.scale);
        if let Some(hit) = self.shape_cache.get(key) {
            return hit;
        }
        let shaped = self.shape_text_attrs_uncached(text, font_size, line_height, max_width, attrs);
        self.shape_cache.insert(key, shaped.clone());
        shaped
    }

    /// Size of `text` under the given metrics / wrap / attrs, without
    /// materialising its glyphs.
    ///
    /// Same inputs, same cache and same answer as
    /// [`shape_text_attrs`](Self::shape_text_attrs) — but a cache hit returns
    /// two floats instead of cloning the whole `ShapedText`. Use this from
    /// `Widget::measure`, which only ever needs the box; use `shape_text_attrs`
    /// when the glyphs themselves are going to be drawn.
    ///
    /// A miss still shapes and populates the cache, so the following paint
    /// finds the glyphs already there.
    pub fn measure_text_attrs(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> (f32, f32) {
        let key = shape_key_plain(text, font_size, line_height, max_width, attrs, self.scale);
        if let Some(dims) = self.shape_cache.dimensions(key) {
            return dims;
        }
        let shaped = self.shape_text_attrs_uncached(text, font_size, line_height, max_width, attrs);
        let dims = (shaped.width, shaped.height);
        self.shape_cache.insert(key, shaped);
        dims
    }

    /// Size of `text` with default attributes — the [`shape_text`](Self::shape_text)
    /// twin of [`measure_text_attrs`](Self::measure_text_attrs).
    pub fn measure_text(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        self.measure_text_attrs(
            text,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Shape plain text with default attributes, **bypassing the cache**.
    ///
    /// For `SecureText`'s transient secret reveal: the shaped glyphs encode the
    /// secret, so they must not be retained in the cache. Masked widgets
    /// (`SecureInput`) shape only the mask string and can use the cached path.
    pub fn shape_text_uncached(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        self.shape_text_attrs_uncached(
            text,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Build a shaped cosmic-text buffer for plain `text` under the given
    /// metrics / wrap / attrs — the single construction point every plain-text
    /// query (`shape_*`, caret, hit-test, selection) goes through, so they all
    /// see the identical wrap configuration by construction.
    fn build_plain_buffer(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> Buffer {
        let t0 = std::time::Instant::now();
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, max_width, None);
        buffer.set_text(
            &mut self.font_system,
            text,
            &attrs.as_cosmic(),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.shape_count += 1;
        self.shape_ns += t0.elapsed().as_nanos() as u64;
        buffer
    }

    /// Shape into positioned glyphs without consulting or populating the cache.
    /// The cached [`shape_text_attrs`](Self::shape_text_attrs) is a thin wrapper
    /// over this.
    fn shape_text_attrs_uncached(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> ShapedText {
        let buffer = self.build_plain_buffer(text, font_size, line_height, max_width, attrs);
        extract_shaped_plain(&buffer, font_size, self.scale)
    }

    /// Shape an inline rich-text run made of `spans`.
    ///
    /// All spans share `font_size` and `line_height` — per-span size is not
    /// modeled (it has no use case in the inline rich text we ship; block-
    /// level size variations are expressed by stacking multiple widgets).
    /// Per-span `family`, `weight`, `style`, and `color` are honored.
    ///
    /// Wraps inside a single span when needed (the whole reason this exists
    /// rather than `Container::row().flex_wrap()` of per-run `TextWidget`s).
    /// `max_width = None` reports the natural max-content width.
    ///
    /// Cached on the full span list (text + attrs + color + decoration) and
    /// metrics — see [`shape_text_attrs`](Self::shape_text_attrs). This is the
    /// hot path for markdown previews, where it spares re-shaping every
    /// paragraph on every idle-tick repaint.
    pub fn shape_rich(
        &mut self,
        spans: &[TextSpan],
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        let key = shape_key_rich(spans, font_size, line_height, max_width, self.scale);
        if let Some(hit) = self.shape_cache.get(key) {
            return hit;
        }
        let shaped = self.shape_rich_uncached(spans, font_size, line_height, max_width);
        self.shape_cache.insert(key, shaped.clone());
        shaped
    }

    /// Size of a rich span run — the [`shape_rich`](Self::shape_rich) twin of
    /// [`measure_text_attrs`](Self::measure_text_attrs), and the one that
    /// matters most for a markdown preview, where every paragraph is a rich
    /// run measured several times per frame.
    pub fn measure_rich(
        &mut self,
        spans: &[TextSpan],
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let key = shape_key_rich(spans, font_size, line_height, max_width, self.scale);
        if let Some(dims) = self.shape_cache.dimensions(key) {
            return dims;
        }
        let shaped = self.shape_rich_uncached(spans, font_size, line_height, max_width);
        let dims = (shaped.width, shaped.height);
        self.shape_cache.insert(key, shaped);
        dims
    }

    /// Build a shaped cosmic-text buffer for a rich span run — the rich twin
    /// of [`build_plain_buffer`](Self::build_plain_buffer).
    fn build_rich_buffer(
        &mut self,
        spans: &[TextSpan],
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> Buffer {
        let t0 = std::time::Instant::now();
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, max_width, None);

        // Build cosmic-text spans on the fly. The iterator borrows from
        // `spans` (for the text and the family name), which lives through the
        // call — `set_rich_text` consumes the iterator immediately.
        let default_attrs = TextAttrs::default();
        let default_cosmic = default_attrs.as_cosmic();
        // Tag each span's glyphs with its index via cosmic-text's per-glyph
        // `metadata`. After shaping we read it back off each `LayoutGlyph` to
        // group glyphs into per-span, per-line bounding boxes (`span_boxes`),
        // which is how the widget maps a click back to the span it landed on.
        let cosmic_spans = spans.iter().enumerate().map(|(i, s)| {
            let mut a = s.attrs.as_cosmic().metadata(i);
            if let Some(c) = s.color {
                a = a.color(shroud_to_cosmic(c));
            }
            (s.text.as_str(), a)
        });
        buffer.set_rich_text(
            &mut self.font_system,
            cosmic_spans,
            &default_cosmic,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.shape_count += 1;
        self.shape_ns += t0.elapsed().as_nanos() as u64;
        buffer
    }

    /// Shape a rich run without consulting or populating the cache. The cached
    /// [`shape_rich`](Self::shape_rich) is a thin wrapper over this.
    fn shape_rich_uncached(
        &mut self,
        spans: &[TextSpan],
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        let buffer = self.build_rich_buffer(spans, font_size, line_height, max_width);
        extract_shaped_rich(&buffer, spans, font_size, self.scale)
    }

    /// Compute the visual position of the cursor sitting at the *end* of
    /// `text_before_cursor`, under the same wrap configuration the caller
    /// uses for full-text rendering.
    ///
    /// Returns `(cursor_x, cursor_line_top_y)`:
    /// - `cursor_x` — horizontal offset within the shaped block, in pixels.
    ///   When the cursor sits at end-of-text or end-of-line, this is the
    ///   right edge of the last visible run. Empty / pure-whitespace
    ///   prefixes return `0.0`.
    /// - `cursor_line_top_y` — top of the cursor's line within the block.
    ///   Multi-line callers stack lines starting at `0.0`, so this is
    ///   directly usable as `text_y_offset + cursor_line_top_y`.
    ///
    /// Hard breaks (`\n`) and soft wraps (`max_width` overflow) are both
    /// honored: the cursor lands on the correct visual line. A prefix
    /// ending in `\n` reports the empty line below (cursor_x = 0, y on the
    /// next baseline).
    pub fn cursor_position(
        &mut self,
        text_before_cursor: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        self.cursor_position_attrs(
            text_before_cursor,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Like [`cursor_position`](Self::cursor_position) but with explicit font
    /// attributes. Use the attrs that match the caller's `shape_text_attrs`
    /// call — passing different attrs here from the render path produces a
    /// cursor offset that does not line up with the painted glyphs.
    pub fn cursor_position_attrs(
        &mut self,
        text_before_cursor: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> (f32, f32) {
        if text_before_cursor.is_empty() {
            return (0.0, 0.0);
        }

        let buffer =
            self.build_plain_buffer(text_before_cursor, font_size, line_height, max_width, attrs);

        // cosmic-text 0.18 yields one run per visual line — including a
        // zero-width run for an empty BufferLine produced by a trailing
        // `\n` (verified via `tests/cursor_probe.rs`). That means the
        // last run already sits at the right `line_top` and reports
        // `line_w = 0` for the empty line, so the cursor falls into place
        // with no extra fix-up. `line_height` is therefore unused here.
        let _ = line_height;
        let mut cursor_x = 0.0;
        let mut cursor_y = 0.0;
        for run in buffer.layout_runs() {
            cursor_x = run.line_w;
            cursor_y = run.line_top;
        }

        (cursor_x, cursor_y)
    }

    /// Map a point within a shaped text block back to a byte offset into
    /// `text` — the inverse of [`cursor_position`](Self::cursor_position).
    ///
    /// `(x, y)` are in the block's local coordinate space (origin at the
    /// block's top-left, the same space `cursor_position` *returns*). The
    /// result is the byte offset of the insertion point nearest that point,
    /// always on a `char` boundary. Used by an editable `Input` to place the
    /// caret where the user clicked and to drag-select.
    ///
    /// `x` / `y` are clamped to the block's leading edge, so a click in the
    /// left / top padding maps to the line start rather than off the front;
    /// cosmic-text's hit test already pins a click past the last line to the
    /// end of the final run. `max_width` must match the wrap configuration
    /// used to render the text or the offset won't line up with the painted
    /// glyphs. Empty `text` returns 0.
    pub fn offset_at_point(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> usize {
        self.offset_at_point_attrs(
            text,
            x,
            y,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Like [`offset_at_point`](Self::offset_at_point) but shaping with explicit
    /// font attributes. Pass the same `attrs` the caller renders with — a
    /// heavier weight advances glyphs differently, so hit-testing under a
    /// mismatched weight lands the caret off the painted glyphs.
    #[allow(clippy::too_many_arguments)]
    pub fn offset_at_point_attrs(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> usize {
        if text.is_empty() {
            return 0;
        }
        let buffer = self.build_plain_buffer(text, font_size, line_height, max_width, attrs);

        let hx = x.max(0.0);
        let hy = y.max(0.0);
        match buffer.hit(hx, hy) {
            Some(cursor) => line_index_to_offset(text, cursor.line, cursor.index),
            None => text.len(),
        }
    }

    /// Block-relative highlight rectangles covering the byte range
    /// `[start, end)` of `text`, one per visual line the range spans.
    ///
    /// Used to back caret geometry ([`cursor_position`](Self::cursor_position))
    /// and the IME preedit underline — callers that need the glyph span
    /// *exactly*, with no trailing affordance. To paint an on-screen
    /// selection, prefer [`selection_rects_with_trailing`](Self::selection_rects_with_trailing).
    ///
    /// Coordinates are in the same block-relative space as
    /// [`cursor_position`](Self::cursor_position) and the painted glyphs
    /// (origin at the block top-left), so a caller adds the text origin to
    /// translate into screen space. `max_width` must match the render wrap
    /// configuration. An empty or inverted range, or empty `text`, returns
    /// an empty `Vec`.
    pub fn selection_rects(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> Vec<Rect> {
        self.selection_rects_impl(
            text,
            start,
            end,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
            false,
        )
    }

    /// Like [`selection_rects`](Self::selection_rects) but shaping with explicit
    /// font attributes — pass the attrs the caller renders with so the rects
    /// line up with the painted glyphs (a heavier weight advances differently).
    #[allow(clippy::too_many_arguments)]
    pub fn selection_rects_attrs(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> Vec<Rect> {
        self.selection_rects_impl(
            text,
            start,
            end,
            font_size,
            line_height,
            max_width,
            attrs,
            false,
        )
    }

    /// Like [`selection_rects`](Self::selection_rects), but every visual
    /// row whose selection continues onto the next row also gets a small
    /// trailing sliver at its right edge. Without it a multi-line selection
    /// stops at each row's last glyph, so the user can't tell whether the
    /// line break (or soft-wrap joint) is part of the selection (FW-6).
    ///
    /// This is the variant `Input` paints behind the glyphs. The plain
    /// [`selection_rects`](Self::selection_rects) stays sliver-free because
    /// caret geometry and the preedit underline reuse it and must not gain
    /// a phantom trailing mark.
    pub fn selection_rects_with_trailing(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> Vec<Rect> {
        self.selection_rects_impl(
            text,
            start,
            end,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
            true,
        )
    }

    /// Like [`selection_rects_with_trailing`](Self::selection_rects_with_trailing)
    /// but shaping with explicit font attributes — the variant an `Input` with a
    /// non-default weight paints its selection with.
    #[allow(clippy::too_many_arguments)]
    pub fn selection_rects_with_trailing_attrs(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> Vec<Rect> {
        self.selection_rects_impl(
            text,
            start,
            end,
            font_size,
            line_height,
            max_width,
            attrs,
            true,
        )
    }

    // Shared body for the public `selection_rects*` methods: the 7-arg shape
    // plus explicit `attrs` and a `trailing` mode flag, so the arg count is
    // over the lint.
    #[allow(clippy::too_many_arguments)]
    fn selection_rects_impl(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
        trailing: bool,
    ) -> Vec<Rect> {
        if text.is_empty() || start >= end {
            return Vec::new();
        }
        let buffer = self.build_plain_buffer(text, font_size, line_height, max_width, attrs);
        let sliver_w = trailing.then_some(font_size * TRAILING_SELECTION_EM);
        selection_rects_in_buffer(&buffer, text, start, end, sliver_w)
    }

    /// Block-relative `(x, y)` of the caret at byte `offset` into `text`,
    /// shaped as one block so soft wraps are accounted for — the
    /// position-by-offset companion to [`offset_at_point`](Self::offset_at_point).
    ///
    /// Unlike [`cursor_position`](Self::cursor_position), which shapes only the
    /// prefix `text[..offset]` and therefore reports the *end of the prefix's*
    /// last line, this shapes the full `text`. An `offset` at a soft-wrap
    /// boundary lands at the **start of the next visual row** — where the glyph
    /// at `offset` actually sits — instead of the end of the previous row,
    /// which is what a vertical caret move / a click on a wrapped row needs.
    /// `max_width` must match the render wrap configuration. Coordinates are in
    /// the same block-relative space as the painted glyphs. Empty `text`
    /// returns `(0, 0)`.
    pub fn caret_at_offset(
        &mut self,
        text: &str,
        offset: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        self.caret_at_offset_attrs(
            text,
            offset,
            font_size,
            line_height,
            max_width,
            &TextAttrs::default(),
        )
    }

    /// Like [`caret_at_offset`](Self::caret_at_offset) but shaping with explicit
    /// font attributes — pass the attrs the caller renders with so the caret
    /// tracks the painted glyphs under a non-default weight.
    #[allow(clippy::too_many_arguments)]
    pub fn caret_at_offset_attrs(
        &mut self,
        text: &str,
        offset: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        // Shape the block once, then read the caret off it. The composing /
        // editing paint paths reuse the same `caret_from_buffer` on a buffer
        // they already hold, so all callers stay bit-for-bit identical.
        let buffer = self.build_plain_buffer(text, font_size, line_height, max_width, attrs);
        self.caret_from_buffer(
            &buffer,
            text,
            offset,
            font_size,
            line_height,
            max_width,
            attrs,
        )
    }

    /// Caret `(x, y)` at byte `offset` derived from an **already-shaped**
    /// `buffer` over `text`. Factored out of
    /// [`caret_at_offset_attrs`](Self::caret_at_offset_attrs) so the IME
    /// composing path can share one shaping across glyphs, caret, and underline.
    ///
    /// Mirrors the original three-branch logic exactly:
    /// * `off < len` with a glyph at `off` → the leading edge of that glyph,
    ///   read via the same `highlight([off, next))` the standalone path used, so
    ///   soft-wrap affinity (caret at the *start of the next row*) is preserved.
    /// * `off == len` → the end of the last visual row, taken straight off this
    ///   buffer's final run (identical to `cursor_position(text)` since the
    ///   prefix is the whole string), with **no extra shape**.
    /// * `off < len` but no glyph at `off` (the caret sits before a hard `\n`) →
    ///   the rare fallback that shapes the shorter prefix `text[..off]`.
    #[allow(clippy::too_many_arguments)]
    fn caret_from_buffer(
        &mut self,
        buffer: &Buffer,
        text: &str,
        offset: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let off = offset.min(text.len());
        if off < text.len() {
            let mut next = off + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            let (lo_line, lo_idx) = offset_to_line_index(text, off);
            let (hi_line, hi_idx) = offset_to_line_index(text, next);
            let cursor_lo = Cursor::new(lo_line, lo_idx);
            let cursor_hi = Cursor::new(hi_line, hi_idx);
            for run in buffer.layout_runs() {
                if let Some((x, w)) = run.highlight(cursor_lo, cursor_hi) {
                    if w > 0.0 {
                        return (x, run.line_top);
                    }
                }
            }
            // No positive-width glyph at `off`. The common cause: the caret
            // rests at the end of a hard line, so `off` points at the `\n`
            // itself, which shapes to nothing highlightable. The historical
            // answer — shape the prefix `text[..off]` and read the end of its
            // last run — is exactly the right edge of `lo_line`'s last visual
            // row: the prefix ends at that complete hard line, and a line's
            // wrap never depends on the lines after it. This buffer already
            // holds those rows, so read the answer off it instead of paying a
            // whole prefix re-shape every frame the caret sits at a line end.
            if text.as_bytes()[off] == b'\n' {
                let mut row_end: Option<(f32, f32)> = None;
                for run in buffer.layout_runs() {
                    if run.line_i == lo_line {
                        row_end = Some((run.line_w, run.line_top));
                    }
                }
                if let Some(xy) = row_end {
                    return xy;
                }
            }
            // Some other zero-width glyph (exotic): keep the prefix-shape
            // fallback for exactness.
            return self.cursor_position_attrs(
                &text[..off],
                font_size,
                line_height,
                max_width,
                attrs,
            );
        }
        // `off == len`: end of the last visual row. `cursor_position(text)`
        // walks every run keeping the last one's `(line_w, line_top)`; the
        // shared buffer holds exactly those runs, so read them directly.
        let mut cursor_x = 0.0;
        let mut cursor_y = 0.0;
        for run in buffer.layout_runs() {
            cursor_x = run.line_w;
            cursor_y = run.line_top;
        }
        (cursor_x, cursor_y)
    }

    /// Shape the IME-composition display string **once** and return the glyphs,
    /// the caret at `caret_offset`, the underline rects for the preedit
    /// `underline` range, and (when `target_range` is a non-empty `[ts, te)`)
    /// the highlight rects for the converting *target clause* — everything a
    /// composing `Input` paints. See [`ComposedBlock`]. Still not routed
    /// through the shape cache: each keystroke's composed string is unique, so
    /// caching would only churn the LRU with transient entries.
    ///
    /// **Incremental**, on the same persistent buffer as the editing paths.
    /// This is the Japanese-input path — composition is what an IME does — and
    /// it was the last one still rebuilding the whole document every frame, so
    /// a long note fell back to pre-incremental cost the moment conversion
    /// started: measured at 3.95 ms per frame at 64 paragraphs and 16.1 ms at
    /// 256, against a 6.06 ms budget (`ime_composing` bench). Sharing the slot
    /// with the editing paths also makes the commit transition nearly free —
    /// the committed value usually equals the last preedit's composed string,
    /// so the first frame after commit re-shapes nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn shape_composing(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
        caret_offset: usize,
        underline_range: (usize, usize),
        target_range: Option<(usize, usize)>,
    ) -> std::rc::Rc<ComposedBlock> {
        let key = shape_key_composing(
            text,
            font_size,
            line_height,
            max_width,
            attrs,
            self.scale,
            caret_offset,
            underline_range,
            target_range,
        );
        if let Some((memo_key, block)) = self.compose_memo.as_ref() {
            if *memo_key == key {
                return std::rc::Rc::clone(block);
            }
        }
        let mut buffer = self.take_edit_buffer(font_size, line_height, max_width);
        let t0 = std::time::Instant::now();
        let changed = sync_edit_lines(&mut buffer, text, &attrs.as_cosmic());
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.record_shape(t0, changed);
        let shaped = extract_shaped_plain(&buffer, font_size, self.scale);

        // Preedit underline (the non-trailing selection body), target-clause
        // highlight, and caret — all off the same buffer, so the highlight
        // lines up with the glyphs and underline exactly. The highlight reuses
        // the selection-rect body (full line-height rects) for the caller to
        // fill like a text selection.
        let underline =
            selection_rects_in_buffer(&buffer, text, underline_range.0, underline_range.1, None);
        let target = match target_range {
            Some((ts, te)) if ts < te => selection_rects_in_buffer(&buffer, text, ts, te, None),
            _ => Vec::new(),
        };
        let caret = self.caret_from_buffer(
            &buffer,
            text,
            caret_offset,
            font_size,
            line_height,
            max_width,
            attrs,
        );
        self.edit_slot = Some(buffer);
        self.slot_key = Some(key);

        let block = std::rc::Rc::new(ComposedBlock {
            shaped,
            caret,
            underline,
            target,
        });
        self.compose_memo = Some((key, std::rc::Rc::clone(&block)));
        block
    }

    /// Shape a focused `Input`'s plain value into the engine's persistent edit
    /// buffer and derive everything the non-composing focused paint needs from
    /// it — glyphs, content height, caret, selection, click hit-tests. See
    /// [`EditBuffer`].
    ///
    /// **Incremental.** The buffer survives between frames and only the lines
    /// whose text actually changed are re-shaped, so typing inside one line of
    /// a long note costs one line's shaping rather than the whole document's.
    /// A repaint whose inputs are unchanged skips even the line walk and hands
    /// back the previous frame's glyphs — the common case by a wide margin,
    /// since most frames in an editing session change nothing.
    /// Still not routed through the shape cache — every keystroke's value is a
    /// unique key, so caching would only churn the LRU; the reuse lives in the
    /// buffer's per-line caches instead.
    pub fn shape_edit_plain(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> EditBuffer {
        let key = shape_key_plain(text, font_size, line_height, max_width, attrs, self.scale);
        if let Some(shaped) = self.edit_memo_hit(key) {
            return EditBuffer { shaped };
        }
        let mut buffer = self.take_edit_buffer(font_size, line_height, max_width);
        let t0 = std::time::Instant::now();
        let changed = sync_edit_lines(&mut buffer, text, &attrs.as_cosmic());
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.record_shape(t0, changed);
        let shaped = std::rc::Rc::new(extract_shaped_plain(&buffer, font_size, self.scale));
        self.edit_slot = Some(buffer);
        self.slot_key = Some(key);
        self.edit_memo = Some((key, std::rc::Rc::clone(&shaped)));
        EditBuffer { shaped }
    }

    /// The glyphs from the previous frame, when `key` says nothing that affects
    /// them has changed since.
    ///
    /// Requires the parked buffer to hold *this* text (`slot_key`): the caret /
    /// hit-test / selection queries read the buffer, not the memo, so handing
    /// back cached glyphs while the slot describes some other string would pair
    /// correct glyphs with a geometry source answering about something else.
    /// A dead slot fails the same check, since `slot_key` is cleared with it.
    ///
    /// Checking the slot's own key rather than merely that it exists is what
    /// makes cancelling an IME composition safe: `value` is then byte-identical
    /// to before the composition, so this memo's key matches, but the slot
    /// still holds the composed string until the miss re-syncs it.
    fn edit_memo_hit(&self, key: u64) -> Option<std::rc::Rc<ShapedText>> {
        let (memo_key, shaped) = self.edit_memo.as_ref()?;
        (*memo_key == key && self.slot_key == Some(key)).then(|| std::rc::Rc::clone(shaped))
    }

    /// Take the persistent edit buffer — creating an empty one when the slot is
    /// cold — and bring its metrics and wrap width up to date.
    ///
    /// A metrics or width change re-lays out the lines already in the buffer
    /// but **keeps their shaping**: cosmic-text shapes independently of font
    /// size, and only layout consumes the metrics (`Buffer::relayout` resets
    /// `layout_opt` while leaving `shape_opt` alone). Unchanged metrics are a
    /// no-op.
    fn take_edit_buffer(
        &mut self,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> Buffer {
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = self
            .edit_slot
            .take()
            .unwrap_or_else(|| Buffer::new_empty(metrics));
        buffer.set_metrics_and_size(&mut self.font_system, metrics, max_width, None);
        // `Buffer::set_text` resets the scroll; the incremental path replaces
        // it, so do the same. Our buffers are unbounded in height and never
        // scroll, making this a no-op in practice.
        buffer.set_scroll(Scroll::default());
        buffer
    }

    /// Fold one edit-buffer build into the perf counters.
    ///
    /// `changed` is how many lines actually had to be re-shaped: a frame that
    /// re-shaped nothing is not a "shape" as far as the frame log is concerned,
    /// which is what keeps `shapes > 0` meaning "real shaping happened" now
    /// that the common keystroke re-shapes only part of the document.
    fn record_shape(&mut self, started: std::time::Instant, changed: usize) {
        if changed > 0 {
            self.shape_count += 1;
        }
        self.reshaped_lines += changed as u32;
        self.shape_ns += started.elapsed().as_nanos() as u64;
    }

    /// Drain the count of lines the incremental editing path re-shaped since
    /// the last call.
    ///
    /// Unlike [`take_shape_stats`](Self::take_shape_stats), which counts whole
    /// buffer builds, this counts the *lines* inside them that actually had to
    /// be re-shaped — the direct read-out of whether an edit is being served
    /// incrementally. Typing one character into a long note should report `1`;
    /// a number near the document's line count means the reuse broke.
    pub fn take_reshaped_lines(&mut self) -> u32 {
        let lines = self.reshaped_lines;
        self.reshaped_lines = 0;
        lines
    }

    /// Rich twin of [`shape_edit_plain`](Self::shape_edit_plain) for a field
    /// with a live highlighter: shapes the span tiling of the value instead.
    /// Color-only spans shape to the identical layout as the plain value (see
    /// the `Input` highlighter invariant), so caret / hit / selection queries
    /// against this buffer agree with the plain-attrs standalone methods.
    ///
    /// Incremental on the same persistent buffer as the plain path — this is
    /// the path a highlighted editor actually takes, so it is the one that
    /// decides what a keystroke in a long note costs. See `sync_edit_lines_rich`
    /// for the two ways the rich line model differs from the plain one.
    pub fn shape_edit_rich(
        &mut self,
        spans: &[TextSpan],
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> EditBuffer {
        let key = shape_key_rich(spans, font_size, line_height, max_width, self.scale);
        if let Some(shaped) = self.edit_memo_hit(key) {
            return EditBuffer { shaped };
        }
        let mut buffer = self.take_edit_buffer(font_size, line_height, max_width);
        let t0 = std::time::Instant::now();
        let default_attrs = TextAttrs::default();
        let changed = sync_edit_lines_rich(&mut buffer, spans, &default_attrs.as_cosmic());
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.record_shape(t0, changed);
        let shaped = std::rc::Rc::new(extract_shaped_rich(&buffer, spans, font_size, self.scale));
        self.edit_slot = Some(buffer);
        self.slot_key = Some(key);
        self.edit_memo = Some((key, std::rc::Rc::clone(&shaped)));
        EditBuffer { shaped }
    }

    /// Block-relative caret at byte `offset`, read off the live edit buffer —
    /// matches [`caret_at_offset_attrs`](Self::caret_at_offset_attrs)
    /// bit-for-bit (both run through `caret_from_buffer`). Falls back to a
    /// standalone shape when no edit buffer is live, so the answer is always
    /// the same one; only the cost differs. `text` must be the string the
    /// buffer was shaped from.
    #[allow(clippy::too_many_arguments)]
    pub fn edit_caret(
        &mut self,
        text: &str,
        offset: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        attrs: &TextAttrs,
    ) -> (f32, f32) {
        // Lend the buffer out of the slot for the call: `caret_from_buffer`
        // needs `&mut self` for the rare zero-width-glyph prefix re-shape.
        let Some(buffer) = self.edit_slot.take() else {
            return self.caret_at_offset_attrs(
                text,
                offset,
                font_size,
                line_height,
                max_width,
                attrs,
            );
        };
        let caret = self.caret_from_buffer(
            &buffer,
            text,
            offset,
            font_size,
            line_height,
            max_width,
            attrs,
        );
        self.edit_slot = Some(buffer);
        caret
    }

    /// Byte offset of the insertion point nearest block-local `(x, y)` in the
    /// live edit buffer, matching
    /// [`offset_at_point_attrs`](Self::offset_at_point_attrs). `None` when no
    /// edit buffer is live (nothing focused, or the slot was dropped on a
    /// screen swap) — the caller then goes through the standalone path.
    /// `text` must be the string the buffer was shaped from (for a rich
    /// buffer, the concatenation of its span texts).
    pub fn edit_hit(&self, text: &str, x: f32, y: f32) -> Option<usize> {
        let buffer = self.edit_slot.as_ref()?;
        if text.is_empty() {
            return Some(0);
        }
        Some(match buffer.hit(x.max(0.0), y.max(0.0)) {
            Some(cursor) => line_index_to_offset(text, cursor.line, cursor.index),
            None => text.len(),
        })
    }

    /// Selection rects for `[start, end)` over the live edit buffer, including
    /// the FW-6 trailing sliver on rows whose selection continues past their
    /// last glyph — matching
    /// [`selection_rects_with_trailing_attrs`](Self::selection_rects_with_trailing_attrs).
    /// `None` when no edit buffer is live. `text` must be the string the buffer
    /// was shaped from.
    pub fn edit_selection_rects_with_trailing(
        &self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
    ) -> Option<Vec<Rect>> {
        let buffer = self.edit_slot.as_ref()?;
        Some(selection_rects_in_buffer(
            buffer,
            text,
            start,
            end,
            Some(font_size * TRAILING_SELECTION_EM),
        ))
    }

    /// Rasterize a glyph into an atlas-ready bitmap.
    ///
    /// Monochrome and subpixel glyphs become single-channel alpha masks
    /// (`is_color == false`). Color emoji keep their full RGBA — extracting
    /// only alpha here is exactly what made them paint as a solid text-color
    /// silhouette ("white emoji"); the renderer now has an RGBA atlas to hold
    /// them. Returns `None` if the glyph has no visible pixels (e.g. space).
    pub fn rasterize(&mut self, cache_key: cosmic_text::CacheKey) -> Option<GlyphImage> {
        let image = self.swash_cache.get_image(&mut self.font_system, cache_key);

        let image = image.as_ref()?;
        let placement = image.placement;

        let (data, is_color) = match image.content {
            SwashContent::Mask => (image.data.clone(), false),
            SwashContent::SubpixelMask => {
                // Subpixel: 3 bytes per pixel (RGB). Average to single alpha.
                let alpha = image
                    .data
                    .chunks_exact(3)
                    .map(|rgb| ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8)
                    .collect();
                (alpha, false)
            }
            // Keep the full RGBA (straight alpha) so the color glyph atlas can
            // render the emoji in its own colors.
            SwashContent::Color => (image.data.clone(), true),
        };

        if placement.width == 0 || placement.height == 0 {
            return None;
        }

        Some(GlyphImage {
            data,
            width: placement.width,
            height: placement.height,
            left: placement.left,
            top: placement.top,
            is_color,
        })
    }
}

/// Split a global byte `offset` into (hard-line index, byte index within that
/// line). Hard lines are `\n`-separated and the `\n` belongs to neither line,
/// matching cosmic-text's `BufferLine` model — so the result feeds directly
/// into `Cursor::new(line, index)`.
fn offset_to_line_index(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line = prefix.matches('\n').count();
    let index = match prefix.rfind('\n') {
        Some(nl) => offset - (nl + 1),
        None => offset,
    };
    (line, index)
}

/// Trailing-sliver width as a fraction of the font size — roughly a
/// space advance, enough to read as "the break is selected" without
/// looking like a stray glyph.
const TRAILING_SELECTION_EM: f32 = 0.33;

/// Rewrite `buffer`'s lines to spell `text`, touching only the lines that
/// differ, and return how many were reset.
///
/// Mirrors `Buffer::set_text` exactly — the same `LineIter` split, the same
/// per-line `AttrsList`, the same trailing line with no ending — except that it
/// reuses the `BufferLine`s already in the buffer. `BufferLine::set_text`
/// compares text + ending + attrs before touching anything, so an unchanged
/// line keeps its cached shaping *and* layout, while `Buffer::set_text` clears
/// the whole vector. That difference is the entire optimization: a keystroke
/// resets one line instead of every line in the document.
///
/// Equivalence with `Buffer::set_text` is the contract here — the incremental
/// buffer must be indistinguishable from a freshly built one, or the caret,
/// hit-tests and glyphs computed from it drift apart from the standalone paths.
fn sync_edit_lines(buffer: &mut Buffer, text: &str, attrs: &cosmic_text::Attrs<'_>) -> usize {
    let mut count = 0usize;
    let mut changed = 0usize;

    for (range, ending) in LineIter::new(text) {
        if set_or_push_line(buffer, count, &text[range], ending, AttrsList::new(attrs)) {
            changed += 1;
        }
        count += 1;
    }

    // `Buffer::set_text` guarantees a final line that carries no ending, so a
    // text ending in `\n` gets an empty last row — and empty text gets exactly
    // one empty line (its `lines.last()` is `None`, whose default ending is
    // `Lf`, so the empty line is pushed).
    let last_ending = match count.checked_sub(1) {
        Some(i) => buffer.lines[i].ending(),
        None => LineEnding::default(),
    };
    if last_ending != LineEnding::None {
        if set_or_push_line(buffer, count, "", LineEnding::None, AttrsList::new(attrs)) {
            changed += 1;
        }
        count += 1;
    }

    // Surplus lines from a shorter text. Dropping them wipes their plaintext —
    // the vendored cosmic-text zeroizes `BufferLine.text` on drop.
    buffer.lines.truncate(count);
    changed
}

/// Overwrite line `index` in place when it exists, otherwise append it.
/// Returns whether the line's cached shaping was reset (a no-op rewrite of an
/// identical line returns `false`, which is the case worth being fast).
fn set_or_push_line(
    buffer: &mut Buffer,
    index: usize,
    text: &str,
    ending: LineEnding,
    attrs_list: AttrsList,
) -> bool {
    match buffer.lines.get_mut(index) {
        Some(line) => line.set_text(text, ending, attrs_list),
        None => {
            buffer
                .lines
                .push(BufferLine::new(text, ending, attrs_list, Shaping::Advanced));
            true
        }
    }
}

/// Rich twin of [`sync_edit_lines`]: rewrite `buffer`'s lines to spell the
/// concatenation of `spans`, touching only the lines that differ.
///
/// Mirrors `Buffer::set_rich_text`, **including the two ways the rich line
/// model differs from the plain one** — get either wrong and the reused buffer
/// stops matching a freshly built one:
///
/// 1. Paragraphs are split with `BidiParagraphs`, not `LineIter`. It yields the
///    line *contents* (separators dropped) and swallows a trailing empty
///    paragraph.
/// 2. Every line is stamped with `LineEnding::default()` rather than its real
///    ending, so the trailing empty line can't be keyed off the last line's
///    ending the way `sync_edit_lines` does — it is keyed off the source
///    string's last character instead, exactly as the fork's `set_rich_text`
///    does. Without it the rich buffer is one line shorter than the plain one
///    for any value ending in a break, and a caret can't reach the final blank
///    line.
///
/// Note what makes a line "differ" here: the per-line `AttrsList` carries each
/// span's *index* as metadata (that is how `extract_shaped_rich` groups glyphs
/// back into span boxes). So an edit that changes how many spans precede a line
/// invalidates it even when its glyphs would be identical — correct, but
/// pessimistic. Ordinary typing inside an existing span leaves the indices
/// alone, which is the case that matters.
fn sync_edit_lines_rich<'a>(
    buffer: &mut Buffer,
    spans: &'a [TextSpan],
    default_attrs: &Attrs<'_>,
) -> usize {
    // Concatenate the run and remember each span's byte range, tagging its
    // attrs with the span index the way `build_rich_buffer` does.
    let mut string = String::new();
    let mut ranges: Vec<((usize, usize), Attrs<'a>)> = Vec::with_capacity(spans.len());
    for (i, s) in spans.iter().enumerate() {
        let start = string.len();
        string.push_str(&s.text);
        let mut a = s.attrs.as_cosmic().metadata(i);
        if let Some(c) = s.color {
            a = a.color(shroud_to_cosmic(c));
        }
        ranges.push(((start, string.len()), a));
    }

    let string_start = string.as_ptr() as usize;
    let mut count = 0usize;
    let mut changed = 0usize;

    // Both lines and spans run left to right, so walk them together rather
    // than intersecting every span with every line — a highlighted document
    // has a span per word, and the quadratic version costs more than the
    // shaping this whole function exists to avoid.
    let mut first_span = 0usize;

    for line in BidiParagraphs::new(&string) {
        let line_start = line.as_ptr() as usize - string_start;
        let line_end = line_start + line.len();

        // Spans that end at or before this line's start can never be reached
        // again. A span straddling the boundary ends *after* it, so it stays.
        while first_span < ranges.len() && ranges[first_span].0.1 <= line_start {
            first_span += 1;
        }

        // Intersect the overlapping spans with this line. Ranges are
        // line-relative, and a span matching the defaults is skipped — both
        // matching what `set_rich_text` builds.
        let mut attrs_list = AttrsList::new(default_attrs);
        for ((span_start, span_end), attrs) in ranges[first_span..]
            .iter()
            .take_while(|((span_start, _), _)| *span_start < line_end)
        {
            let start = (*span_start).max(line_start);
            let end = (*span_end).min(line_end);
            if start < end && *attrs != attrs_list.defaults() {
                attrs_list.add_span(start - line_start..end - line_start, attrs);
            }
        }

        if set_or_push_line(buffer, count, line, LineEnding::default(), attrs_list) {
            changed += 1;
        }
        count += 1;
    }

    // `BidiParagraphs` yields nothing at all for an empty run, but
    // `set_rich_text` still emits one empty line — its "reached only if this
    // text is empty" branch. An empty field is one row tall, not zero, and the
    // caret has to have a row to sit on.
    if count == 0 {
        if set_or_push_line(
            buffer,
            0,
            "",
            LineEnding::default(),
            AttrsList::new(default_attrs),
        ) {
            changed += 1;
        }
        count = 1;
    } else if matches!(string.chars().next_back(), Some('\n' | '\r')) {
        if set_or_push_line(
            buffer,
            count,
            "",
            LineEnding::None,
            AttrsList::new(default_attrs),
        ) {
            changed += 1;
        }
        count += 1;
    }

    buffer.lines.truncate(count);
    changed
}

/// Selection/underline rects for the cursor range `[start, end)` over an
/// **already-shaped** `buffer` — the shared body of the `selection_rects*`
/// family, factored out so a caller that already holds a shaped buffer (the
/// IME composing path's underline, [`EditBuffer`]'s selection) gets the rects
/// without re-shaping. Keeping every caller on this one body is what lets them
/// match the standalone methods exactly.
///
/// `sliver_w` adds the FW-6 trailing sliver of that width to every visual row
/// whose selection continues onto the next one; `None` keeps the rects
/// glyph-tight (caret geometry and the preedit underline reuse those and must
/// not gain a phantom trailing mark).
fn selection_rects_in_buffer(
    buffer: &Buffer,
    text: &str,
    start: usize,
    end: usize,
    sliver_w: Option<f32>,
) -> Vec<Rect> {
    if text.is_empty() || start >= end {
        return Vec::new();
    }
    let lo = start.min(text.len());
    let hi = end.min(text.len());
    let (lo_line, lo_idx) = offset_to_line_index(text, lo);
    let (hi_line, hi_idx) = offset_to_line_index(text, hi);
    let cursor_lo = Cursor::new(lo_line, lo_idx);
    let cursor_hi = Cursor::new(hi_line, hi_idx);

    // Byte offset where each `\n`-delimited line starts, indexed by the
    // cosmic-text buffer line index (`run.line_i`). Lets us map a run's
    // per-line glyph byte ranges back into global offsets for the
    // trailing-sliver test. Only needed when the sliver is requested.
    let line_starts: Vec<usize> = if sliver_w.is_some() {
        let mut v = vec![0usize];
        v.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        v
    } else {
        Vec::new()
    };

    let mut rects = Vec::new();
    for run in buffer.layout_runs() {
        // `highlight` returns the pixel span of this run's glyphs whose
        // cursor falls within [lo, hi]. A run with no intersecting glyphs
        // (e.g. a blank line between the endpoints) yields None.
        if let Some((x, w)) = run.highlight(cursor_lo, cursor_hi) {
            if w > 0.0 {
                rects.push(Rect::new(x, run.line_top, w, run.line_height));
            }
        }

        // Trailing sliver: when the selection reaches past this visual
        // row's last glyph, the line break / soft-wrap joint after it is
        // selected, so draw a small block at the row's right edge. The
        // glyph byte ranges are line-relative, so rebase through
        // `line_starts[run.line_i]` to compare against the global
        // [lo, hi). An empty run (blank line) ends at the line start and
        // anchors the sliver at the left edge.
        if let Some(sliver_w) = sliver_w {
            let line_start = line_starts.get(run.line_i).copied().unwrap_or(0);
            let (row_end_global, row_right_x) = match run.glyphs.last() {
                Some(g) => (line_start + g.end, g.x + g.w),
                None => (line_start, 0.0),
            };
            if lo <= row_end_global && row_end_global < hi {
                rects.push(Rect::new(
                    row_right_x,
                    run.line_top,
                    sliver_w,
                    run.line_height,
                ));
            }
        }
    }
    rects
}

/// Inverse of [`offset_to_line_index`]: resolve a cosmic-text
/// `(line, byte-in-line)` cursor back to a global byte offset into `text`,
/// clamped to the text length and snapped down to a `char` boundary (so a hit
/// landing inside a multi-byte codepoint never produces a non-boundary offset).
fn line_index_to_offset(text: &str, line: usize, index: usize) -> usize {
    let mut line_start = 0;
    for _ in 0..line {
        match text[line_start..].find('\n') {
            Some(rel) => line_start += rel + 1,
            None => return text.len(),
        }
    }
    let mut target = (line_start + index).min(text.len());
    while target > 0 && !text.is_char_boundary(target) {
        target -= 1;
    }
    target
}

/// Where the text baseline sits inside the em box, as a fraction of
/// `font_size` measured from the box top. Most Latin/CJK faces put the baseline
/// around 0.8 of the em down; using one constant for every line is what makes
/// the baseline script-independent (see [`stable_baseline`]).
const BASELINE_ASCENT_RATIO: f32 = 0.8;

/// Baseline Y for a run, replacing cosmic-text's per-line `LayoutRun::line_y`.
///
/// cosmic-text derives `line_y` from the *actual* fonts shaped on that line
/// (`line_top + (line_height − (max_ascent + max_descent))/2 + max_ascent`), so
/// the baseline moves when a line's font mix changes — e.g. an ASCII-only run
/// and a CJK-containing run pick up different fallback fonts and so sit on
/// different baselines, which showed up as `[a]`'s brackets sitting lower than
/// `[あ]`'s. We instead treat the em (`font_size`) as a fixed content box,
/// center it in `line_height` (CSS half-leading style), and put the baseline a
/// fixed [`BASELINE_ASCENT_RATIO`] of the way down it. The result depends only
/// on the metrics every line shares, so every line lands on one baseline.
fn stable_baseline(line_top: f32, line_height: f32, font_size: f32) -> f32 {
    line_top + (line_height - font_size) / 2.0 + BASELINE_ASCENT_RATIO * font_size
}

/// Read positioned glyphs and the block extent off an already-shaped plain
/// buffer — the extraction half of the plain shape, shared by
/// `shape_text_attrs_uncached` and the single-shape editing paths
/// ([`ComposedBlock`], [`EditBuffer`]) so their glyphs match the standalone
/// shape bit-for-bit.
fn extract_shaped_plain(buffer: &Buffer, font_size: f32, scale: f32) -> ShapedText {
    let mut glyphs = Vec::new();
    let mut total_width: f32 = 0.0;
    let mut total_height: f32 = 0.0;

    for run in buffer.layout_runs() {
        total_width = total_width.max(run.line_w);
        total_height = run.line_top + run.line_height;

        // Place glyphs on a *script-independent* baseline rather than
        // cosmic-text's `run.line_y`. cosmic centers the baseline using the
        // line's actual fonts' ascent/descent, so a line's baseline jumps
        // when its script mix changes — ASCII-only `[a]` and CJK-containing
        // `[あ]` resolve different fallback fonts and so landed on different
        // baselines (the brackets in `[a]` sat visibly lower). See
        // [`stable_baseline`]. The downstream renderer subtracts
        // `image.top` from `glyph.y`, so this is still a baseline Y.
        let baseline = stable_baseline(run.line_top, run.line_height, font_size);
        for glyph in run.glyphs.iter() {
            // `scale` makes text crisp above 100%: it snaps the glyph to the
            // physical pixel grid and stamps `font_size * scale` into the cache
            // key, so the rasterizer renders the outline at device resolution.
            // Shaping stays logical, so nothing here moves a line break or a
            // caret — only how finely the same glyph is drawn.
            //
            // The offset must be *pre-scaled*: `physical()` multiplies the
            // glyph's own position by `scale` but adds the offset as-is (it
            // expects a physical pen origin), so a logical baseline here would
            // survive the later divide-by-scale at half value and drag every
            // glyph upward. `x` is 0, which is why only the baseline shows it.
            let physical = glyph.physical((0.0, baseline * scale), scale);
            glyphs.push(ShapedGlyph {
                cache_key: physical.cache_key,
                // Back into logical space. The scale survives in `cache_key`,
                // which is what the rasterizer reads, so the bitmap is still cut
                // at device resolution — only the *coordinate* is logical, which
                // it must be: callers add a logical origin to it.
                x: physical.x as f32 / scale,
                y: physical.y as f32 / scale,
                color: None,
            });
        }
    }

    ShapedText {
        glyphs,
        width: total_width,
        height: total_height,
        span_boxes: Vec::new(),
        decoration_lines: Vec::new(),
    }
}

/// Rich twin of [`extract_shaped_plain`]: glyphs carry per-span colors, and
/// per-span / per-line boxes plus decoration lines are grouped back together
/// via the glyph `metadata` (= span index) planted at buffer build time.
fn extract_shaped_rich(
    buffer: &Buffer,
    spans: &[TextSpan],
    font_size: f32,
    scale: f32,
) -> ShapedText {
    let mut glyphs = Vec::new();
    let mut span_boxes: Vec<SpanBox> = Vec::new();
    let mut decoration_lines: Vec<DecorationLine> = Vec::new();
    let mut total_width: f32 = 0.0;
    let mut total_height: f32 = 0.0;

    for run in buffer.layout_runs() {
        total_width = total_width.max(run.line_w);
        total_height = run.line_top + run.line_height;
        // Script-independent baseline, replacing cosmic-text's `run.line_y`
        // (see the plain path and [`stable_baseline`]). Used both to place
        // glyphs and to anchor decoration lines, so under/strike-through
        // track the repositioned glyphs.
        let baseline = stable_baseline(run.line_top, run.line_height, font_size);
        // Accumulate the horizontal extent of consecutive glyphs sharing a
        // metadata (= span index) on this line. A group is flushed whenever
        // the span changes and at end-of-line, so each span gets one box
        // (and any decoration line) per visual line it occupies.
        let mut group: Option<(usize, f32, f32)> = None; // (span, min_x, max_x)
        for glyph in run.glyphs.iter() {
            let color = glyph.color_opt.map(cosmic_to_shroud);
            // `scale` makes text crisp above 100%: it snaps the glyph to the
            // physical pixel grid and stamps `font_size * scale` into the cache
            // key, so the rasterizer renders the outline at device resolution.
            // Shaping stays logical, so nothing here moves a line break or a
            // caret — only how finely the same glyph is drawn.
            //
            // The offset must be *pre-scaled*: `physical()` multiplies the
            // glyph's own position by `scale` but adds the offset as-is (it
            // expects a physical pen origin), so a logical baseline here would
            // survive the later divide-by-scale at half value and drag every
            // glyph upward. `x` is 0, which is why only the baseline shows it.
            let physical = glyph.physical((0.0, baseline * scale), scale);
            glyphs.push(ShapedGlyph {
                cache_key: physical.cache_key,
                // Back into logical space. The scale survives in `cache_key`,
                // which is what the rasterizer reads, so the bitmap is still cut
                // at device resolution — only the *coordinate* is logical, which
                // it must be: callers add a logical origin to it.
                x: physical.x as f32 / scale,
                y: physical.y as f32 / scale,
                color,
            });

            let x0 = glyph.x;
            let x1 = glyph.x + glyph.w;
            match group {
                Some((m, min, max)) if m == glyph.metadata => {
                    group = Some((m, min.min(x0), max.max(x1)));
                }
                _ => {
                    if let Some((m, min, max)) = group.take() {
                        flush_group(
                            &mut span_boxes,
                            &mut decoration_lines,
                            spans,
                            m,
                            min,
                            max,
                            run.line_top,
                            run.line_height,
                            baseline,
                            font_size,
                        );
                    }
                    group = Some((glyph.metadata, x0, x1));
                }
            }
        }
        if let Some((m, min, max)) = group.take() {
            flush_group(
                &mut span_boxes,
                &mut decoration_lines,
                spans,
                m,
                min,
                max,
                run.line_top,
                run.line_height,
                baseline,
                font_size,
            );
        }
    }

    ShapedText {
        glyphs,
        width: total_width,
        height: total_height,
        span_boxes,
        decoration_lines,
    }
}

/// Emit the span box for a finished glyph group, plus any decoration lines the
/// group's span asks for.
///
/// A "group" is a maximal run of same-span glyphs on one visual line;
/// `min_x`/`max_x` bracket its horizontal extent. `line_top`/`line_height`
/// frame the span box (used for click hit-testing); `baseline` is the line's
/// baseline Y, from which decoration lines are placed geometrically (cosmic-text
/// doesn't shape decorations). Offsets are fractions of `font_size` so they
/// scale with the text:
/// - underline sits ~0.12·size below the baseline,
/// - strike-through is centered ~0.26·size above the baseline (near the middle
///   of the x-height),
/// - thickness is ~0.07·size, at least 1px so it never rounds away.
#[allow(clippy::too_many_arguments)]
fn flush_group(
    span_boxes: &mut Vec<SpanBox>,
    decoration_lines: &mut Vec<DecorationLine>,
    spans: &[TextSpan],
    span: usize,
    min_x: f32,
    max_x: f32,
    line_top: f32,
    line_height: f32,
    baseline: f32,
    font_size: f32,
) {
    let width = max_x - min_x;
    span_boxes.push(SpanBox {
        span,
        rect: Rect::new(min_x, line_top, width, line_height),
    });

    let Some(s) = spans.get(span) else { return };
    let deco = s.decoration;
    if width <= 0.0 || !(deco.underline || deco.strikethrough) {
        return;
    }
    let thickness = (font_size * 0.07).round().max(1.0);
    let color = s.color;
    if deco.underline {
        let y = (baseline + font_size * 0.12).round();
        decoration_lines.push(DecorationLine {
            rect: Rect::new(min_x, y, width, thickness),
            color,
        });
    }
    if deco.strikethrough {
        let y = (baseline - font_size * 0.26 - thickness / 2.0).round();
        decoration_lines.push(DecorationLine {
            rect: Rect::new(min_x, y, width, thickness),
            color,
        });
    }
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    //! Cache mechanics, exercised against private internals. Observable shaping
    //! behavior (cache hits return correct results, every key dimension is
    //! honored) is covered by the integration tests in `text_engine_tests.rs`.
    use super::*;

    fn empty_shaped() -> ShapedText {
        ShapedText {
            glyphs: Vec::new(),
            width: 0.0,
            height: 0.0,
            span_boxes: Vec::new(),
            decoration_lines: Vec::new(),
        }
    }

    #[test]
    fn shape_cache_dedups_keys_and_clears() {
        let mut c = ShapeCache::new();
        assert_eq!(c.map.len(), 0);
        c.insert(1, empty_shaped());
        c.insert(1, empty_shaped()); // same key — replaces, no growth
        assert_eq!(c.map.len(), 1);
        c.insert(2, empty_shaped());
        assert_eq!(c.map.len(), 2);
        assert!(c.get(1).is_some());
        assert!(c.get(99).is_none());
        c.clear();
        assert_eq!(c.map.len(), 0);
    }

    #[test]
    fn shape_cache_evicts_least_recently_used_past_cap() {
        let mut c = ShapeCache::new();
        for k in 0..SHAPE_CACHE_CAP as u64 {
            c.insert(k, empty_shaped());
        }
        assert_eq!(c.map.len(), SHAPE_CACHE_CAP);
        // Touch key 0 so it becomes most-recently-used; key 1 is now the LRU.
        assert!(c.get(0).is_some());
        // A brand-new key evicts exactly the LRU and holds the map at the cap.
        c.insert(u64::MAX, empty_shaped());
        assert_eq!(c.map.len(), SHAPE_CACHE_CAP);
        assert!(c.get(0).is_some(), "recently-touched entry must survive");
        assert!(
            c.get(1).is_none(),
            "least-recently-used entry must be evicted"
        );
        assert!(c.get(u64::MAX).is_some(), "new entry must be present");
    }

    #[test]
    fn cached_path_populates_uncached_path_does_not() {
        let mut e = TextEngine::new();
        assert_eq!(e.shape_cache.map.len(), 0);

        let a = e.shape_text("Hello", 16.0, 20.0, None);
        assert_eq!(e.shape_cache.map.len(), 1, "cached shape must populate");

        // Identical inputs hit the cache: same result, no growth.
        let b = e.shape_text("Hello", 16.0, 20.0, None);
        assert_eq!(e.shape_cache.map.len(), 1);
        assert_eq!(a.glyphs.len(), b.glyphs.len());
        assert_eq!(a.width, b.width);

        // The secret-reveal path must never deposit glyph geometry in the cache.
        let _ = e.shape_text_uncached("Secret", 16.0, 20.0, None);
        assert_eq!(e.shape_cache.map.len(), 1, "uncached must not populate");

        e.clear_shape_cache();
        assert_eq!(e.shape_cache.map.len(), 0);
    }

    #[test]
    fn edit_buffers_do_not_populate_the_cache() {
        // The focused editing path shapes a unique string per keystroke;
        // caching those would only churn the LRU with transient entries (and
        // `shape_composing`'s may carry preedit text). All three single-shape
        // paths must leave the cache untouched.
        let mut e = TextEngine::new();
        let attrs = TextAttrs::default();
        let _ = e.shape_edit_plain("typing", 16.0, 20.0, None, &attrs);
        let _ = e.shape_edit_rich(&[TextSpan::new("typing")], 16.0, 20.0, None);
        let _ = e.shape_composing("typing", 16.0, 20.0, None, &attrs, 3, (0, 3), None);
        assert_eq!(
            e.shape_cache.map.len(),
            0,
            "single-shape editing paths must not populate the cache"
        );
    }
}
