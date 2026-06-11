//! Core text engine: shaping + rasterization.

use crate::attrs::TextAttrs;
use crate::span::{TextSpan, cosmic_to_shroud, shroud_to_cosmic};
use cosmic_text::{Buffer, Cursor, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use shroud_core::{Color, Rect};

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
#[derive(Debug)]
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

/// A rasterized glyph image (alpha mask).
pub struct GlyphImage {
    /// Alpha data, one byte per pixel.
    pub data: Vec<u8>,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// X bearing (offset from glyph origin to left edge of bitmap).
    pub left: i32,
    /// Y bearing (offset from glyph origin to top edge of bitmap).
    pub top: i32,
}

impl std::fmt::Debug for GlyphImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("left", &self.left)
            .field("top", &self.top)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Text engine: owns the font system and glyph cache.
///
/// Create one per application. Not Send — cosmic-text's FontSystem is not
/// thread-safe.
pub struct TextEngine {
    font_system: FontSystem,
    swash_cache: SwashCache,
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
        }
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
    pub fn shape_text_attrs(
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

            // `run.line_y` is the baseline Y within the shaped block. We must
            // pass it as the offset to `physical()` so that each glyph's `.y`
            // is its screen-space baseline (relative to the block top) rather
            // than a meaningless 0 — the downstream renderer subtracts
            // `image.top` from `glyph.y` expecting baseline coordinates.
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((0.0, run.line_y), 1.0);
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
    pub fn shape_rich(
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
            // Accumulate the horizontal extent of consecutive glyphs sharing a
            // metadata (= span index) on this line. A group is flushed whenever
            // the span changes and at end-of-line, so each span gets one box
            // (and any decoration line) per visual line it occupies.
            let mut group: Option<(usize, f32, f32)> = None; // (span, min_x, max_x)
            for glyph in run.glyphs.iter() {
                let color = glyph.color_opt.map(cosmic_to_shroud);
                let physical = glyph.physical((0.0, run.line_y), 1.0);
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
                                run.line_y,
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
                    run.line_y,
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
    /// Used by `Input` to paint a selection behind the glyphs. Coordinates
    /// are in the same block-relative space as [`cursor_position`] and the
    /// painted glyphs (origin at the block top-left), so a caller adds the
    /// text origin to translate into screen space. `max_width` must match
    /// the render wrap configuration. An empty or inverted range, or empty
    /// `text`, returns an empty `Vec`.
    pub fn selection_rects(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> Vec<Rect> {
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
        }
        rects
    }

    /// Rasterize a glyph into an alpha mask.
    ///
    /// Returns `None` if the glyph has no visible pixels (e.g. space).
    pub fn rasterize(&mut self, cache_key: cosmic_text::CacheKey) -> Option<GlyphImage> {
        let image = self.swash_cache.get_image(&mut self.font_system, cache_key);

        let image = image.as_ref()?;
        let placement = image.placement;

        // Convert to alpha mask regardless of source format
        let alpha_data = match image.content {
            SwashContent::Mask => image.data.clone(),
            SwashContent::SubpixelMask => {
                // Subpixel: 3 bytes per pixel (RGB). Average to single alpha.
                image
                    .data
                    .chunks_exact(3)
                    .map(|rgb| ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8)
                    .collect()
            }
            SwashContent::Color => {
                // RGBA: extract alpha channel
                image.data.chunks_exact(4).map(|rgba| rgba[3]).collect()
            }
        };

        if placement.width == 0 || placement.height == 0 {
            return None;
        }

        Some(GlyphImage {
            data: alpha_data,
            width: placement.width,
            height: placement.height,
            left: placement.left,
            top: placement.top,
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
