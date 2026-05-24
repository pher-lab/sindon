//! Paint context — accumulates draw commands during widget painting.

use std::sync::Arc;

use shroud_core::{Color, Rect, Theme};
use shroud_render::{DecodedImage, DrawGlyph, DrawImage, DrawRect, LayerSnapshot};
use shroud_text::{GlyphImage, TextEngine};

/// Accumulates draw commands produced by widget `paint()` calls.
///
/// After painting the entire widget tree, the collected rects, glyphs,
/// and images are passed to the Renderer.
///
/// Also provides access to the `TextEngine` for text shaping/rasterization.
pub struct PaintContext {
    pub rects: Vec<DrawRect>,
    pub glyphs: Vec<DrawGlyph>,
    /// Glyphs from secure widgets — rendered via the secure atlas
    /// which is cleared every frame after presentation.
    pub secure_glyphs: Vec<DrawGlyph>,
    /// Images, rendered after rects but before text within each layer
    /// (so an icon paints over its background but a label still sits on
    /// top of the icon if they overlap).
    pub images: Vec<DrawImage>,
    pub text_engine: TextEngine,
    /// Active theme — widgets read defaults from here.
    pub theme: Theme,
    /// Stack of clipping rectangles (already intersected with ancestors).
    clip_stack: Vec<Rect>,
    /// Stack of accumulated translation offsets.
    offset_stack: Vec<(f32, f32)>,
    /// Boundaries between paint *layers* — each entry snapshots the
    /// command-vec lengths at the moment the corresponding overlay
    /// layer started painting.
    ///
    /// Renders the main tree as one logical batch and every overlay
    /// layer as its own; the renderer iterates these spans in z order
    /// so a layer rect never gets overdrawn by the main tree's text.
    /// Empty when no layers were pushed in the current frame.
    layer_starts: Vec<LayerSnapshot>,
    /// Window-relative rectangle of the next-character / caret area for
    /// IME composition. The focused text widget writes this during paint
    /// via [`set_ime_cursor_area`](Self::set_ime_cursor_area); the event
    /// loop then forwards it to the OS so the IME candidate window
    /// anchors near the cursor instead of defaulting to a screen corner.
    ///
    /// `None` when nothing focused needs IME positioning. Reset to
    /// `None` on every frame by [`clear`](Self::clear) — the focused
    /// widget re-establishes it on the very next paint, so a transient
    /// blip during focus transitions stays one frame at most.
    ime_cursor_area: Option<Rect>,
    /// Whether any widget painted this frame asked the OS IME to stay
    /// off for the rest of the frame.
    ///
    /// `SecureInput` flips this when focused so the OS-level IME is
    /// disconnected from the window while a password / master key is
    /// being typed — keystrokes bypass IME entirely and arrive as raw
    /// characters instead of going through a composition window an
    /// IME engine (or a malicious replacement IME) could observe.
    ///
    /// Default `false` (allow IME). Reset every frame by
    /// [`clear`](Self::clear) so a widget that loses focus this frame
    /// gives the IME back next frame; the event loop folds the value
    /// into a `set_ime_allowed(bool)` push after paint with the same
    /// dedup discipline used for [`ime_cursor_area`](Self::ime_cursor_area).
    suppress_ime: bool,
}

impl PaintContext {
    pub fn new(theme: Theme) -> Self {
        Self {
            rects: Vec::new(),
            glyphs: Vec::new(),
            secure_glyphs: Vec::new(),
            images: Vec::new(),
            text_engine: TextEngine::new(),
            theme,
            clip_stack: Vec::new(),
            offset_stack: Vec::new(),
            layer_starts: Vec::new(),
            ime_cursor_area: None,
            suppress_ime: false,
        }
    }

    /// Mark the start of an overlay layer's paint batch. Subsequent
    /// `fill_rect` / `draw_glyph` / `draw_image` calls become part of
    /// this layer's batch, which the renderer draws after every
    /// previously-recorded batch. Called by
    /// [`WidgetTree::paint`](crate::tree::WidgetTree::paint) once per
    /// layer; widget code does not invoke this directly.
    pub fn begin_layer(&mut self) {
        self.layer_starts.push(LayerSnapshot {
            rect: self.rects.len(),
            glyph: self.glyphs.len(),
            secure_glyph: self.secure_glyphs.len(),
            image: self.images.len(),
        });
    }

    /// Read the recorded layer-batch boundaries. Renderer-facing —
    /// each entry is the cumulative command-vec lengths at the start of
    /// one overlay layer.
    pub fn layer_starts(&self) -> &[LayerSnapshot] {
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

    /// Record a draw call for `image` at the given rect, tinted by
    /// `tint` (use [`Color::WHITE`] for unmodified pixels). The active
    /// offset and clip stack are applied automatically.
    pub fn draw_image(&mut self, rect: Rect, image: Arc<DecodedImage>, tint: Color) {
        let (ox, oy) = self.current_offset();
        self.images.push(DrawImage {
            x: rect.origin.x + ox,
            y: rect.origin.y + oy,
            width: rect.size.width,
            height: rect.size.height,
            image,
            tint,
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

    /// Record where the focused text widget's caret currently sits, in
    /// the same widget-paint coordinate space as `fill_rect`
    /// (i.e. the active offset stack is applied here just like draw
    /// calls). The IME then anchors its candidate window near this rect
    /// instead of falling back to a screen-corner default.
    ///
    /// Called by `Input::paint` / `SecureInput::paint` (and any future
    /// text widget) when focused. Last-writer-wins within a frame —
    /// only one input has IME focus at a time, so multiple writes
    /// shouldn't happen in practice; the contract is "the focused
    /// widget's caret rect."
    ///
    /// The event loop reads [`ime_cursor_area`](Self::ime_cursor_area)
    /// after paint and forwards the rect to the platform window.
    pub fn set_ime_cursor_area(&mut self, rect: Rect) {
        let (ox, oy) = self.current_offset();
        self.ime_cursor_area = Some(Rect::new(
            rect.origin.x + ox,
            rect.origin.y + oy,
            rect.size.width,
            rect.size.height,
        ));
    }

    /// Read back the caret rect set by `set_ime_cursor_area` for the
    /// current frame. `None` if no focused text widget recorded one.
    pub fn ime_cursor_area(&self) -> Option<Rect> {
        self.ime_cursor_area
    }

    /// Ask the event loop to disconnect the OS IME from this window for
    /// the rest of the current frame.
    ///
    /// Called by [`SecureInput::paint`](crate::secure_input::SecureInput)
    /// when focused so a password / master-key entry bypasses the IME
    /// entirely — keystrokes arrive as raw chars rather than going through
    /// a composition window the IME engine (or a malicious replacement
    /// IME) could observe. Idempotent within a frame; the event loop
    /// reads back via [`ime_suppressed`](Self::ime_suppressed) once paint
    /// finishes and pushes the result through `set_ime_allowed(!suppress)`.
    ///
    /// Other text widgets (`Input`, future `TextArea`) deliberately do
    /// not call this — IME stays live for them so CJK users can type
    /// composed characters into notes and similar plain text fields.
    pub fn suppress_ime(&mut self) {
        self.suppress_ime = true;
    }

    /// Read back whether any widget asked for IME suppression this frame.
    /// Consumed by the event loop after paint to drive a deduped
    /// `set_ime_allowed(bool)` push on the platform window.
    pub fn ime_suppressed(&self) -> bool {
        self.suppress_ime
    }

    /// Clear all accumulated commands.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.glyphs.clear();
        self.secure_glyphs.clear();
        self.images.clear();
        self.layer_starts.clear();
        self.ime_cursor_area = None;
        self.suppress_ime = false;
    }
}

impl Default for PaintContext {
    fn default() -> Self {
        Self::new(Theme::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_cursor_area_defaults_to_none() {
        // Fresh PaintContext has nothing anchored — the event loop
        // relies on this to skip the OS push when nothing focused.
        let ctx = PaintContext::default();
        assert_eq!(ctx.ime_cursor_area(), None);
    }

    #[test]
    fn ime_cursor_area_round_trips_through_set() {
        // The setter records the rect verbatim when no offset is active;
        // the getter hands it back unchanged. Used by the event loop to
        // diff between frames and dedupe OS calls.
        let mut ctx = PaintContext::default();
        ctx.set_ime_cursor_area(Rect::new(10.0, 20.0, 2.0, 16.0));
        assert_eq!(
            ctx.ime_cursor_area(),
            Some(Rect::new(10.0, 20.0, 2.0, 16.0))
        );
    }

    #[test]
    fn ime_cursor_area_applies_current_offset() {
        // The active offset stack must be folded in just like fill_rect /
        // draw_glyph — a text widget inside a ScrollView or a layer
        // reports its caret in widget-local coords, and the OS needs
        // window-relative coords for the IME anchor to land correctly.
        let mut ctx = PaintContext::default();
        ctx.push_offset(100.0, 50.0);
        ctx.set_ime_cursor_area(Rect::new(10.0, 20.0, 2.0, 16.0));
        assert_eq!(
            ctx.ime_cursor_area(),
            Some(Rect::new(110.0, 70.0, 2.0, 16.0))
        );
        ctx.pop_offset();
    }

    #[test]
    fn clear_resets_ime_cursor_area() {
        // Every frame starts fresh — clear() (called by the tree's paint
        // pass before traversal) must drop the stale anchor so a widget
        // that lost focus this frame doesn't leave the IME pinned to
        // last frame's caret.
        let mut ctx = PaintContext::default();
        ctx.set_ime_cursor_area(Rect::new(10.0, 20.0, 2.0, 16.0));
        ctx.clear();
        assert_eq!(ctx.ime_cursor_area(), None);
    }

    #[test]
    fn ime_suppressed_defaults_to_false() {
        // Fresh PaintContext leaves IME enabled — the event loop reads
        // this on every paint to decide whether to push set_ime_allowed,
        // so the default must match "IME on" (the platform's start state
        // after resumed()).
        let ctx = PaintContext::default();
        assert!(!ctx.ime_suppressed());
    }

    #[test]
    fn suppress_ime_flips_the_flag() {
        // SecureInput.paint calls this when focused; the value is then
        // read by the event loop after paint via ime_suppressed().
        let mut ctx = PaintContext::default();
        ctx.suppress_ime();
        assert!(ctx.ime_suppressed());
    }

    #[test]
    fn suppress_ime_is_idempotent_within_a_frame() {
        // Two SecureInputs in the same tree (or paint passing through the
        // widget twice via some future double-dispatch) must not toggle
        // the flag back off — only `clear()` resets it.
        let mut ctx = PaintContext::default();
        ctx.suppress_ime();
        ctx.suppress_ime();
        assert!(ctx.ime_suppressed());
    }

    #[test]
    fn clear_resets_suppress_ime() {
        // Every frame starts with IME re-allowed. Without this, a
        // SecureInput that lost focus would still leave the OS IME
        // disabled because no widget would re-assert "allow".
        let mut ctx = PaintContext::default();
        ctx.suppress_ime();
        ctx.clear();
        assert!(!ctx.ime_suppressed());
    }
}
