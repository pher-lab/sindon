//! Paint context — accumulates draw commands during widget painting.

use shroud_core::{Color, Rect, Theme};
use shroud_render::{DrawGlyph, DrawRect};
use shroud_text::{GlyphImage, TextEngine};

/// Accumulates draw commands produced by widget `paint()` calls.
///
/// After painting the entire widget tree, the collected rects and glyphs
/// are passed to the Renderer.
///
/// Also provides access to the `TextEngine` for text shaping/rasterization.
pub struct PaintContext {
    pub rects: Vec<DrawRect>,
    pub glyphs: Vec<DrawGlyph>,
    /// Glyphs from secure widgets — rendered via the secure atlas
    /// which is cleared every frame after presentation.
    pub secure_glyphs: Vec<DrawGlyph>,
    pub text_engine: TextEngine,
    /// Active theme — widgets read defaults from here.
    pub theme: Theme,
    /// Stack of clipping rectangles (already intersected with ancestors).
    clip_stack: Vec<Rect>,
    /// Stack of accumulated translation offsets.
    offset_stack: Vec<(f32, f32)>,
}

impl PaintContext {
    pub fn new(theme: Theme) -> Self {
        Self {
            rects: Vec::new(),
            glyphs: Vec::new(),
            secure_glyphs: Vec::new(),
            text_engine: TextEngine::new(),
            theme,
            clip_stack: Vec::new(),
            offset_stack: Vec::new(),
        }
    }

    /// Push a clip rectangle. If a clip is already active, the pushed clip is
    /// intersected with the current clip (nested clipping semantics).
    pub fn push_clip(&mut self, rect: Rect) {
        let effective = match self.clip_stack.last() {
            Some(parent) => rect.intersect(parent).unwrap_or(Rect::ZERO),
            None => rect,
        };
        self.clip_stack.push(effective);
    }

    /// Pop the most recent clip rectangle.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Push a translation offset to be added to subsequent draw positions.
    /// The effective offset is accumulated with any currently active offset.
    pub fn push_offset(&mut self, dx: f32, dy: f32) {
        let (px, py) = self.current_offset();
        self.offset_stack.push((px + dx, py + dy));
    }

    /// Pop the most recent offset.
    pub fn pop_offset(&mut self) {
        self.offset_stack.pop();
    }

    /// Current accumulated offset.
    pub fn current_offset(&self) -> (f32, f32) {
        self.offset_stack.last().copied().unwrap_or((0.0, 0.0))
    }

    /// Current effective clip rectangle, if any.
    pub fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().copied()
    }

    /// Draw a filled rectangle.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let (ox, oy) = self.current_offset();
        self.rects.push(DrawRect {
            x: rect.origin.x + ox,
            y: rect.origin.y + oy,
            width: rect.size.width,
            height: rect.size.height,
            color,
            clip_rect: self.current_clip(),
        });
    }

    /// Draw a pre-rasterized glyph at the given position (standard atlas).
    pub fn draw_glyph(
        &mut self,
        x: i32,
        y: i32,
        image: GlyphImage,
        color: Color,
        cache_key: shroud_text::CacheKey,
    ) {
        let (ox, oy) = self.current_offset();
        self.glyphs.push(DrawGlyph {
            x: x + ox as i32,
            y: y + oy as i32,
            image,
            color,
            cache_key,
            clip_rect: self.current_clip(),
        });
    }

    /// Draw a glyph into the secure atlas (cleared every frame).
    ///
    /// Use this for glyphs from `SecureText` / `SecureInput` widgets.
    pub fn draw_secure_glyph(
        &mut self,
        x: i32,
        y: i32,
        image: GlyphImage,
        color: Color,
        cache_key: shroud_text::CacheKey,
    ) {
        let (ox, oy) = self.current_offset();
        self.secure_glyphs.push(DrawGlyph {
            x: x + ox as i32,
            y: y + oy as i32,
            image,
            color,
            cache_key,
            clip_rect: self.current_clip(),
        });
    }

    /// Clear all accumulated commands.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.glyphs.clear();
        self.secure_glyphs.clear();
    }
}

impl Default for PaintContext {
    fn default() -> Self {
        Self::new(Theme::default())
    }
}
