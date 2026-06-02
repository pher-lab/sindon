//! Core text engine: shaping + rasterization.

use crate::attrs::TextAttrs;
use crate::span::{TextSpan, cosmic_to_shroud, shroud_to_cosmic};
use cosmic_text::{Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
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
        let mut total_width: f32 = 0.0;
        let mut total_height: f32 = 0.0;

        for run in buffer.layout_runs() {
            total_width = total_width.max(run.line_w);
            total_height = run.line_top + run.line_height;
            // Accumulate the horizontal extent of consecutive glyphs sharing a
            // metadata (= span index) on this line. A box is flushed whenever
            // the span changes and at end-of-line, so each span gets one box
            // per visual line it occupies.
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
                            span_boxes.push(SpanBox {
                                span: m,
                                rect: Rect::new(min, run.line_top, max - min, run.line_height),
                            });
                        }
                        group = Some((glyph.metadata, x0, x1));
                    }
                }
            }
            if let Some((m, min, max)) = group.take() {
                span_boxes.push(SpanBox {
                    span: m,
                    rect: Rect::new(min, run.line_top, max - min, run.line_height),
                });
            }
        }

        ShapedText {
            glyphs,
            width: total_width,
            height: total_height,
            span_boxes,
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

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine").finish_non_exhaustive()
    }
}
