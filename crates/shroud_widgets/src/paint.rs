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
    /// Boundaries between paint *layers* — each tuple is the
    /// `(rects.len(), glyphs.len(), secure_glyphs.len())` snapshot at
    /// the moment the corresponding overlay layer started painting.
    ///
    /// Renders the main tree as one logical batch and every overlay
    /// layer as its own; the renderer iterates these spans in z order
    /// so a layer rect never gets overdrawn by the main tree's text.
    /// Empty when no layers were pushed in the current frame.
    layer_starts: Vec<(usize, usize, usize)>,
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
            layer_starts: Vec::new(),
        }
    }

    /// Mark the start of an overlay layer's paint batch. Subsequent
    /// `fill_rect` / `draw_glyph` calls become part of this layer's
    /// batch, which the renderer draws after every previously-recorded
    /// batch. Called by [`WidgetTree::paint`](crate::tree::WidgetTree::paint)
    /// once per layer; widget code does not invoke this directly.
    pub fn begin_layer(&mut self) {
        self.layer_starts.push((
            self.rects.len(),
            self.glyphs.len(),
            self.secure_glyphs.len(),
        ));
    }

    /// Read the recorded layer-batch boundaries. Renderer-facing —
    /// each entry is the cumulative `(rect_count, glyph_count,
    /// secure_glyph_count)` at the start of one overlay layer.
    pub fn layer_starts(&self) -> &[(usize, usize, usize)] {
        &self.layer_starts
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

    /// Draw a filled rectangle with sharp corners.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fill_rect_rounded(rect, color, 0.0);
    }

    /// Draw a filled rectangle with rounded corners.
    ///
    /// `radius` is in pixels. Values `<= 0.0` produce sharp corners and
    /// short-circuit the SDF in the rect shader. The radius is clamped
    /// downstream to half of the smaller side, so callers don't need to
    /// validate against the rect's dimensions.
    pub fn fill_rect_rounded(&mut self, rect: Rect, color: Color, radius: f32) {
        let (ox, oy) = self.current_offset();
        self.rects.push(DrawRect {
            x: rect.origin.x + ox,
            y: rect.origin.y + oy,
            width: rect.size.width,
            height: rect.size.height,
            color,
            radius,
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

    /// Paint a keyboard-focus ring around `widget_rect`.
    ///
    /// The ring sits outside the widget rect with a `theme.focus.ring_offset`
    /// gap before its inner edge, and is `theme.focus.ring_width` thick.
    /// Pass `override_color = None` to use `theme.focus.ring_color`, or
    /// `Some(c)` for a per-widget accent.
    ///
    /// Implemented as four `fill_rect` calls (top / bottom / left / right
    /// strokes) so the active clip and offset stacks are honored — a
    /// focused widget that has scrolled partly out of a `ScrollView` has
    /// its ring clipped consistently with the widget itself.
    pub fn paint_focus_ring(&mut self, widget_rect: Rect, override_color: Option<Color>) {
        let style = self.theme.focus;
        let color = override_color.unwrap_or(style.ring_color);
        let w = style.ring_width;
        let outer_off = style.ring_offset + w;
        let ox = widget_rect.origin.x - outer_off;
        let oy = widget_rect.origin.y - outer_off;
        let width = widget_rect.size.width + 2.0 * outer_off;
        let height = widget_rect.size.height + 2.0 * outer_off;
        self.fill_rect(Rect::new(ox, oy, width, w), color);
        self.fill_rect(Rect::new(ox, oy + height - w, width, w), color);
        self.fill_rect(Rect::new(ox, oy, w, height), color);
        self.fill_rect(Rect::new(ox + width - w, oy, w, height), color);
    }

    /// Clear all accumulated commands.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.glyphs.clear();
        self.secure_glyphs.clear();
        self.layer_starts.clear();
    }
}

impl Default for PaintContext {
    fn default() -> Self {
        Self::new(Theme::default())
    }
}
