//! Core text engine: shaping + rasterization.

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};

/// A positioned glyph ready for rasterization.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Cache key for rasterization lookup.
    pub cache_key: cosmic_text::CacheKey,
    /// Pixel X position (integer, after subpixel binning).
    pub x: i32,
    /// Pixel Y position (integer, after subpixel binning).
    pub y: i32,
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

    /// Shape a text string into positioned glyphs.
    ///
    /// `font_size`: font size in pixels.
    /// `line_height`: line height in pixels.
    /// `max_width`: optional max width for line wrapping.
    /// `text`: the string to shape.
    pub fn shape_text(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);

        buffer.set_size(&mut self.font_system, max_width, None);
        buffer.set_text(
            &mut self.font_system,
            text,
            &Attrs::new(),
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
                });
            }
        }

        ShapedText {
            glyphs,
            width: total_width,
            height: total_height,
        }
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
