//! Core text engine: shaping + rasterization.

use crate::attrs::{FontStyle, TextAttrs};
use crate::span::{TextSpan, cosmic_to_shroud, shroud_to_cosmic};
use cosmic_text::{Buffer, Cursor, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use shroud_core::{Color, Rect};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A positioned glyph ready for rasterization.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Cache key for rasterization lookup.
    pub cache_key: cosmic_text::CacheKey,
    /// Pixel X position (integer, after subpixel binning).
    pub x: i32,
    /// Pixel Y position (integer, after subpixel binning).
    pub y: i32,
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

fn hash_metrics(font_size: f32, line_height: f32, max_width: Option<f32>, h: &mut DefaultHasher) {
    font_size.to_bits().hash(h);
    line_height.to_bits().hash(h);
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
) -> u64 {
    let mut h = DefaultHasher::new();
    0u8.hash(&mut h);
    text.hash(&mut h);
    hash_attrs(attrs, &mut h);
    hash_metrics(font_size, line_height, max_width, &mut h);
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
    hash_metrics(font_size, line_height, max_width, &mut h);
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
        }
    }

    /// Drop every cached shaping result.
    ///
    /// Called on a screen swap (the framework clears it when the tree root is
    /// replaced) so glyph geometry derived from one screen's text — which for a
    /// notes app is the user's plaintext — does not outlive that screen. Also
    /// the escape hatch if a font is registered at runtime and previously-shaped
    /// text should be re-evaluated against it.
    pub fn clear_shape_cache(&mut self) {
        self.shape_cache.clear();
    }

    /// Access the underlying FontSystem (for advanced usage).
    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
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
        let key = shape_key_plain(text, font_size, line_height, max_width, attrs);
        if let Some(hit) = self.shape_cache.get(key) {
            return hit;
        }
        let shaped = self.shape_text_attrs_uncached(text, font_size, line_height, max_width, attrs);
        self.shape_cache.insert(key, shaped.clone());
        shaped
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
                let physical = glyph.physical((0.0, baseline), 1.0);
                glyphs.push(ShapedGlyph {
                    cache_key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
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
        let key = shape_key_rich(spans, font_size, line_height, max_width);
        if let Some(hit) = self.shape_cache.get(key) {
            return hit;
        }
        let shaped = self.shape_rich_uncached(spans, font_size, line_height, max_width);
        self.shape_cache.insert(key, shaped.clone());
        shaped
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
                let physical = glyph.physical((0.0, baseline), 1.0);
                glyphs.push(ShapedGlyph {
                    cache_key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
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

        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, max_width, None);
        buffer.set_text(
            &mut self.font_system,
            text_before_cursor,
            &attrs.as_cosmic(),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

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
        if text.is_empty() {
            return 0;
        }
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, max_width, None);
        buffer.set_text(
            &mut self.font_system,
            text,
            &TextAttrs::default().as_cosmic(),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

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
        self.selection_rects_impl(text, start, end, font_size, line_height, max_width, false)
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
        self.selection_rects_impl(text, start, end, font_size, line_height, max_width, true)
    }

    // Shared body for the two public `selection_rects*` methods: same 7-arg
    // shape plus a `trailing` mode flag, so the arg count is one over the lint.
    #[allow(clippy::too_many_arguments)]
    fn selection_rects_impl(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
        trailing: bool,
    ) -> Vec<Rect> {
        /// Trailing-sliver width as a fraction of the font size — roughly a
        /// space advance, enough to read as "the break is selected" without
        /// looking like a stray glyph.
        const TRAILING_SELECTION_EM: f32 = 0.33;

        if text.is_empty() || start >= end {
            return Vec::new();
        }
        let lo = start.min(text.len());
        let hi = end.min(text.len());
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, max_width, None);
        buffer.set_text(
            &mut self.font_system,
            text,
            &TextAttrs::default().as_cosmic(),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let (lo_line, lo_idx) = offset_to_line_index(text, lo);
        let (hi_line, hi_idx) = offset_to_line_index(text, hi);
        let cursor_lo = Cursor::new(lo_line, lo_idx);
        let cursor_hi = Cursor::new(hi_line, hi_idx);

        // Byte offset where each `\n`-delimited line starts, indexed by the
        // cosmic-text buffer line index (`run.line_i`). Lets us map a run's
        // per-line glyph byte ranges back into global offsets for the
        // trailing-sliver test. Only needed when `trailing` is set.
        let line_starts: Vec<usize> = if trailing {
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
        let sliver_w = font_size * TRAILING_SELECTION_EM;

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
            if trailing {
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
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let off = offset.min(text.len());
        if off < text.len() {
            // The caret sits at the left edge of the glyph that begins at `off`.
            // `selection_rects` shapes the full block and uses cosmic-text's
            // cursor model, so it resolves the wrap-boundary affinity correctly
            // (the glyph at `off` belongs to the next visual row). One char of
            // range is enough to get that glyph's leading edge.
            let mut next = off + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if let Some(r) = self
                .selection_rects(text, off, next, font_size, line_height, max_width)
                .first()
            {
                return (r.origin.x, r.origin.y);
            }
        }
        // End of text, or the next char carries no glyph (a hard `\n`): the end
        // of the prefix's last line is the right answer, and prefix shaping
        // gives it without a wrap-boundary ambiguity.
        self.cursor_position(&text[..off], font_size, line_height, max_width)
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
}
