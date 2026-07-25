//! Paint context — accumulates draw commands during widget painting.

use std::sync::Arc;

use shroud_core::{Color, Point, Rect, Theme};
use shroud_render::{DecodedImage, DrawGlyph, DrawImage, DrawRect, GlyphRotation, LayerSnapshot};
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
    /// Stack of active glyph rotations (each with an *absolute* pivot —
    /// the active offset is folded in at `push_rotation` time). Only glyph
    /// draws consult this; rects and images stay axis-aligned. The
    /// innermost entry wins — rotations are not composed, since the sole
    /// use case (icon glyphs / disclosure chevrons) never nests them.
    rotation_stack: Vec<GlyphRotation>,
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
    /// Whether the currently focused widget should paint a focus ring this
    /// frame (the `:focus-visible` heuristic).
    ///
    /// Set once per frame by [`WidgetTree::paint`](crate::tree::WidgetTree::paint)
    /// from the tree's focus state: `true` when focus was last moved by the
    /// keyboard or programmatically, `false` when it followed a pointer
    /// press. A focused widget gates its `paint_focus_ring` call on this so
    /// a click doesn't leave a ring the user reads as noise. Only one widget
    /// is focused at a time, so a single tree-wide flag is unambiguous.
    ///
    /// Default `false`; reset to `false` by [`clear`](Self::clear) each frame
    /// before the tree re-establishes it.
    focus_visible: bool,
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
            rotation_stack: Vec::new(),
            layer_starts: Vec::new(),
            ime_cursor_area: None,
            suppress_ime: false,
            focus_visible: false,
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
    ///
    /// The active offset is folded in — just like [`fill_rect`](Self::fill_rect),
    /// [`draw_glyph`](Self::draw_glyph), [`push_rotation`](Self::push_rotation),
    /// and [`set_ime_cursor_area`](Self::set_ime_cursor_area) — because the
    /// `rect` is given in the *widget-local* space the caller received in
    /// `paint`, whereas the content it wraps is drawn with the offset applied.
    /// Without this, a clip pushed inside a translated context (a `ScrollView`,
    /// or any clipping widget nested in a centered/anchored layer whose offset
    /// the tree applies before painting the subtree) would scissor against
    /// local coords while its content draws offset — clipping the content away.
    /// The main tree paints at offset `(0, 0)`, so this is a no-op there;
    /// inside a layer it lands the clip in the same absolute space as the draws.
    pub fn push_clip(&mut self, rect: Rect) {
        let (ox, oy) = self.current_offset();
        let rect = Rect::new(
            rect.origin.x + ox,
            rect.origin.y + oy,
            rect.size.width,
            rect.size.height,
        );
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

    /// Push a glyph rotation. Subsequent [`draw_glyph`](Self::draw_glyph) /
    /// [`draw_secure_glyph`](Self::draw_secure_glyph) calls spin their quads
    /// rigidly about `pivot` by `angle` radians (clockwise-positive in screen
    /// space, Y-down — a `▸` chevron at `+PI/2` points down).
    ///
    /// `pivot` is in the same widget-local coordinate space as draw positions;
    /// the active offset is folded in here so the pivot and the glyphs it
    /// turns end up in the same absolute space. Only glyphs are affected —
    /// rects (backgrounds, focus rings, decoration lines) stay axis-aligned.
    ///
    /// Pair every call with [`pop_rotation`](Self::pop_rotation).
    pub fn push_rotation(&mut self, angle: f32, pivot: Point) {
        let (ox, oy) = self.current_offset();
        self.rotation_stack.push(GlyphRotation {
            angle,
            pivot_x: pivot.x + ox,
            pivot_y: pivot.y + oy,
        });
    }

    /// Pop the most recent glyph rotation.
    pub fn pop_rotation(&mut self) {
        self.rotation_stack.pop();
    }

    /// The currently active glyph rotation (innermost), if any.
    pub fn current_rotation(&self) -> Option<GlyphRotation> {
        self.rotation_stack.last().copied()
    }

    /// Current effective clip rectangle, if any.
    pub fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().copied()
    }

    /// Whether a draw quad at *composited* `(x, y, w, h)` would be scissored
    /// away entirely by the active clip — i.e. recording it would cost a
    /// vertex, an atlas slot and a batch entry to produce zero pixels.
    ///
    /// This is the exact converse of what `apply_scissor` does in the
    /// renderer, so dropping such a command is invisible by construction: no
    /// containment assumption about widget layout is involved, only the quad
    /// the renderer would have emitted. It matters because the glyph atlas has
    /// no per-frame bound — a document taller than its `ScrollView` piles every
    /// glyph it contains into the shared atlas even though only a screenful can
    /// ever show, which is what eventually exhausts it and blanks new glyphs
    /// (the culling `Input` already does by hand for its own viewport, here
    /// generalized to every widget under any clip).
    ///
    /// The test is *strict* disjointness rather than an empty intersection, so
    /// a degenerate quad (a zero-area glyph — a space — or a hairline rect)
    /// inside the clip is kept, exactly as before.
    fn clipped_out(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let Some(clip) = self.clip_stack.last() else {
            return false;
        };
        x + w <= clip.origin.x || x >= clip.right() || y + h <= clip.origin.y || y >= clip.bottom()
    }

    /// Round a logical coordinate to the nearest device-pixel boundary, using
    /// the text engine's current scale (physical pixels per logical pixel).
    ///
    /// Rounds in *device* space (scale up, round, scale back) so the result is
    /// exact at fractional DPI (125% / 150%) — rounding in logical space would
    /// miss the physical grid. Intended for thin, view-only elements (the text
    /// caret, hairline decoration rules) whose ~0.5px antialiased rect edge
    /// otherwise straddles two physical columns and smears onto whatever sits
    /// next to it. Snap the *drawn* geometry only — never an offset a hit-test
    /// or selection derives from, or click-to-caret would drift off the glyphs.
    ///
    /// The value passed in must already include any active [`current_offset`]
    /// (snap the composited coordinate, not the widget-local one), so a caller
    /// inside a translated layer stays on the same grid the renderer rasterizes
    /// against.
    ///
    /// [`current_offset`]: Self::current_offset
    pub fn snap_device_px(&self, v: f32) -> f32 {
        let scale = self.text_engine.scale();
        if scale > 0.0 {
            (v * scale).round() / scale
        } else {
            v
        }
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
        self.push_rect(rect, color, radius, 0.0, 0.0);
    }

    /// Draw a `border_width`-thick outline along the inside of `rect`'s
    /// (optionally rounded) edge, leaving the interior transparent.
    ///
    /// A single rect carries the whole stroke — the SDF in the rect
    /// shader keeps only the band between the outer edge and that edge
    /// shifted inward by `border_width`, so the corners round with
    /// `radius` automatically. Used by [`paint_focus_ring`](Self::paint_focus_ring);
    /// `border_width <= 0.0` degenerates to a solid fill.
    pub fn stroke_rect_rounded(
        &mut self,
        rect: Rect,
        color: Color,
        radius: f32,
        border_width: f32,
    ) {
        self.push_rect(rect, color, radius, border_width, 0.0);
    }

    /// Draw a soft drop shadow: a blurred silhouette of `rect` (with corner
    /// `radius`) that fades from full opacity at the box edge to zero `blur`
    /// px outside.
    ///
    /// `rect` is the **casting box** — the caller has already applied any
    /// shadow offset and spread (see [`Container::shadow`](crate::Container::shadow),
    /// which folds `offset_x/offset_y` and `spread` into the rect before
    /// calling this). The interior paints at full `color` alpha, so a widget
    /// drawing an opaque background on top covers it and only the blurred
    /// halo peeking past the box shows — exactly a CSS `box-shadow`.
    ///
    /// `blur <= 0.0` is a no-op: a shadow with no blur is just a rect, and
    /// silently emitting a solid fill here would surprise callers. The active
    /// offset and clip stacks are applied like every other draw call, so a
    /// shadow inside a translated layer or a `ScrollView` lands and clips with
    /// its owner.
    pub fn fill_shadow(&mut self, rect: Rect, color: Color, radius: f32, blur: f32) {
        if blur <= 0.0 {
            return;
        }
        self.push_rect(rect, color, radius, 0.0, blur);
    }

    fn push_rect(&mut self, rect: Rect, color: Color, radius: f32, border_width: f32, blur: f32) {
        let (ox, oy) = self.current_offset();
        // A blurred shadow inflates its quad by `blur` on every side (the
        // renderer does the same when it builds the geometry), so grow the
        // cull box to match or a halo reaching into the clip would vanish.
        let m = blur.max(0.0);
        if self.clipped_out(
            rect.origin.x + ox - m,
            rect.origin.y + oy - m,
            rect.size.width + 2.0 * m,
            rect.size.height + 2.0 * m,
        ) {
            return;
        }
        self.rects.push(DrawRect {
            x: rect.origin.x + ox,
            y: rect.origin.y + oy,
            width: rect.size.width,
            height: rect.size.height,
            color,
            radius,
            border_width,
            blur,
            clip_rect: self.current_clip(),
        });
    }

    /// Draw a pre-rasterized glyph at the given position (standard atlas).
    pub fn draw_glyph(
        &mut self,
        x: f32,
        y: f32,
        image: GlyphImage,
        color: Color,
        cache_key: shroud_text::CacheKey,
    ) {
        let (ox, oy) = self.current_offset();
        let rotation = self.current_rotation();
        let (x, y) = self.snap_glyph_origin(x + ox, y + oy, rotation.is_some());
        if self.glyph_clipped_out(x, y, &image, rotation.is_some()) {
            return;
        }
        self.glyphs.push(DrawGlyph {
            x,
            y,
            image,
            color,
            cache_key,
            clip_rect: self.current_clip(),
            rotation,
        });
    }

    /// Snap a glyph's *composited* origin to the device-pixel grid, unless the
    /// glyph is being rotated.
    ///
    /// The text engine rasterizes glyphs at device resolution and quantizes
    /// each glyph's run-relative position to a whole device pixel, baking the
    /// sub-pixel remainder into the bitmap (the subpixel bin — see
    /// `shroud_text`). That keeps a run crisp *only while its origin also sits
    /// on the grid*. But a widget's run origin is a raw logical layout
    /// coordinate, and flex centering, `grow`, and gaps routinely land a
    /// `Text` / `Button` label on a fractional pixel — even at 100% DPI — which
    /// smears the crisp bitmap across two pixels (uniformly soft labels, while
    /// `Input`, which already snapped its own origin, stayed sharp). Snapping
    /// the composited origin shifts the whole run by one sub-pixel amount, so
    /// every glyph lands on the grid; because the run-relative advances are
    /// already whole device pixels, inter-glyph spacing is preserved exactly.
    /// Rotated glyphs (icon chevrons) are placed by rotation math about a
    /// pivot, so leave them untouched. View-only: hit-testing and selection
    /// read the unsnapped shaped geometry, matching the caret's rule.
    fn snap_glyph_origin(&self, x: f32, y: f32, rotated: bool) -> (f32, f32) {
        if rotated {
            (x, y)
        } else {
            (self.snap_device_px(x), self.snap_device_px(y))
        }
    }

    /// Whether the quad the renderer would build for this glyph falls entirely
    /// outside the active clip. See [`clipped_out`](Self::clipped_out).
    ///
    /// The quad is reconstructed exactly as `build_glyph_geometry` does: the
    /// bitmap is rasterized at device resolution, so its size and bearings
    /// divide by the text scale to land back in the logical space the clip
    /// lives in. A *rotated* glyph is never culled — its quad is placed by
    /// rotation math about a pivot, so the axis-aligned box computed here does
    /// not describe where it actually lands.
    fn glyph_clipped_out(&self, x: f32, y: f32, image: &GlyphImage, rotated: bool) -> bool {
        if rotated || self.clip_stack.is_empty() {
            return false;
        }
        let scale = self.text_engine.scale();
        if scale <= 0.0 {
            return false;
        }
        let inv = 1.0 / scale;
        self.clipped_out(
            x + image.left as f32 * inv,
            y - image.top as f32 * inv,
            image.width as f32 * inv,
            image.height as f32 * inv,
        )
    }

    /// Record a draw call for `image` at the given rect, tinted by
    /// `tint` (use [`Color::WHITE`] for unmodified pixels). The active
    /// offset and clip stack are applied automatically.
    pub fn draw_image(&mut self, rect: Rect, image: Arc<DecodedImage>, tint: Color) {
        let (ox, oy) = self.current_offset();
        if self.clipped_out(
            rect.origin.x + ox,
            rect.origin.y + oy,
            rect.size.width,
            rect.size.height,
        ) {
            return;
        }
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
        x: f32,
        y: f32,
        image: GlyphImage,
        color: Color,
        cache_key: shroud_text::CacheKey,
    ) {
        let (ox, oy) = self.current_offset();
        let rotation = self.current_rotation();
        let (x, y) = self.snap_glyph_origin(x + ox, y + oy, rotation.is_some());
        if self.glyph_clipped_out(x, y, &image, rotation.is_some()) {
            return;
        }
        self.secure_glyphs.push(DrawGlyph {
            x,
            y,
            image,
            color,
            cache_key,
            clip_rect: self.current_clip(),
            rotation,
        });
    }

    /// Paint a keyboard-focus ring around `widget_rect`.
    ///
    /// The ring sits outside the widget rect with a `theme.focus.ring_offset`
    /// gap before its inner edge, and is `theme.focus.ring_width` thick.
    /// Pass `override_color = None` to use `theme.focus.ring_color`, or
    /// `Some(c)` for a per-widget accent.
    ///
    /// `widget_radius` is the widget's own corner radius (`0.0` for a
    /// square widget); the ring's outer corner is grown to
    /// `widget_radius + offset + width` so a rounded widget gets a
    /// concentric rounded ring and a square one keeps square corners.
    ///
    /// Emitted as a single stroked rect so the active clip and offset
    /// stacks are honored — a focused widget that has scrolled partly out
    /// of a `ScrollView` has its ring clipped consistently with the widget
    /// itself.
    pub fn paint_focus_ring(
        &mut self,
        widget_rect: Rect,
        override_color: Option<Color>,
        widget_radius: f32,
    ) {
        let style = self.theme.focus;
        let color = override_color.unwrap_or(style.ring_color);
        let w = style.ring_width;
        let outer_off = style.ring_offset + w;
        let ox = widget_rect.origin.x - outer_off;
        let oy = widget_rect.origin.y - outer_off;
        let width = widget_rect.size.width + 2.0 * outer_off;
        let height = widget_rect.size.height + 2.0 * outer_off;
        // Keep the ring concentric: grow the radius by the gap between the
        // widget edge and the ring's outer edge. A square widget (radius 0)
        // stays square.
        let outer_radius = if widget_radius > 0.0 {
            widget_radius + outer_off
        } else {
            0.0
        };
        self.stroke_rect_rounded(Rect::new(ox, oy, width, height), color, outer_radius, w);
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

    /// Set whether the focused widget should paint its focus ring this
    /// frame. Called once per frame by the tree's paint pass from its
    /// focus state; widget code reads it via [`focus_visible`](Self::focus_visible).
    pub fn set_focus_visible(&mut self, visible: bool) {
        self.focus_visible = visible;
    }

    /// Whether the focused widget should paint a focus ring (the
    /// `:focus-visible` heuristic). A focused widget gates its
    /// [`paint_focus_ring`](Self::paint_focus_ring) call on this so
    /// click-to-focus doesn't show a ring while Tab focus does.
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Clear all accumulated commands.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.glyphs.clear();
        self.secure_glyphs.clear();
        self.images.clear();
        self.layer_starts.clear();
        self.rotation_stack.clear();
        self.ime_cursor_area = None;
        self.suppress_ime = false;
        self.focus_visible = false;
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
    fn rotation_defaults_to_none() {
        // A fresh context has no active rotation — glyph draws stay
        // axis-aligned, which the renderer short-circuits.
        let ctx = PaintContext::default();
        assert_eq!(ctx.current_rotation(), None);
    }

    #[test]
    fn push_rotation_folds_in_the_active_offset() {
        // The pivot is given in widget-local coords; push_rotation adds the
        // active offset so the pivot lives in the same absolute space as the
        // glyph positions draw_glyph emits (which also add the offset).
        let mut ctx = PaintContext::default();
        ctx.push_offset(100.0, 50.0);
        ctx.push_rotation(std::f32::consts::FRAC_PI_2, Point::new(10.0, 20.0));
        let rot = ctx.current_rotation().expect("rotation active");
        assert!((rot.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!((rot.pivot_x, rot.pivot_y), (110.0, 70.0));
        ctx.pop_rotation();
        assert_eq!(ctx.current_rotation(), None);
        ctx.pop_offset();
    }

    #[test]
    fn push_clip_folds_in_the_active_offset() {
        // A clip is given in widget-local coords, but the content it wraps is
        // drawn with the active offset applied (fill_rect / draw_glyph add it).
        // So push_clip must fold the offset in too, or a ScrollView / clipping
        // widget nested in a translated layer would scissor against local
        // coords and clip its own offset-drawn content away. Regression guard
        // for the Restore-modal bug: a centered layer's offset (~316px) left
        // the clip at x=0 while glyphs drew at x=332+, hiding 400+ glyphs.
        let mut ctx = PaintContext::default();
        ctx.push_offset(100.0, 50.0);
        ctx.push_clip(Rect::new(10.0, 20.0, 200.0, 80.0));
        assert_eq!(
            ctx.current_clip(),
            Some(Rect::new(110.0, 70.0, 200.0, 80.0)),
            "clip origin must be shifted by the active offset"
        );
        // A rect drawn at the same local origin lands inside the clip — the
        // two now share one absolute space.
        ctx.fill_rect(Rect::new(10.0, 20.0, 5.0, 5.0), Color::WHITE);
        let r = ctx.rects.last().unwrap();
        let clip = r.clip_rect.unwrap();
        assert!(
            r.x >= clip.origin.x && r.x < clip.origin.x + clip.size.width,
            "drawn rect x={} should sit inside its clip x={}..{}",
            r.x,
            clip.origin.x,
            clip.origin.x + clip.size.width
        );
        ctx.pop_clip();
        ctx.pop_offset();
    }

    #[test]
    fn push_clip_no_offset_is_unchanged() {
        // Main-tree paint runs at offset (0,0): the fold must be a no-op there
        // so existing (offset-free) clip math is preserved.
        let mut ctx = PaintContext::default();
        ctx.push_clip(Rect::new(10.0, 10.0, 80.0, 80.0));
        assert_eq!(ctx.current_clip(), Some(Rect::new(10.0, 10.0, 80.0, 80.0)));
        ctx.pop_clip();
    }

    #[test]
    fn innermost_rotation_wins() {
        // Rotations don't compose; the most recent push is the active one,
        // and popping it restores the previous.
        let mut ctx = PaintContext::default();
        ctx.push_rotation(0.5, Point::new(0.0, 0.0));
        ctx.push_rotation(1.5, Point::new(0.0, 0.0));
        assert!((ctx.current_rotation().unwrap().angle - 1.5).abs() < 1e-6);
        ctx.pop_rotation();
        assert!((ctx.current_rotation().unwrap().angle - 0.5).abs() < 1e-6);
        ctx.pop_rotation();
    }

    #[test]
    fn clear_resets_rotation_stack() {
        // Every frame starts unrotated even if a widget left a rotation on
        // the stack (it never should, but clear is the safety net).
        let mut ctx = PaintContext::default();
        ctx.push_rotation(1.0, Point::new(5.0, 5.0));
        ctx.clear();
        assert_eq!(ctx.current_rotation(), None);
    }

    #[test]
    fn fill_shadow_records_blur_and_folds_offset() {
        // A shadow emitted inside a translated context (a centered modal
        // layer) must land in the same absolute space as the card it backs —
        // the active offset is folded in just like fill_rect. blur is carried
        // through so the renderer inflates + softens the quad.
        let mut ctx = PaintContext::default();
        ctx.push_offset(100.0, 50.0);
        ctx.fill_shadow(Rect::new(10.0, 20.0, 200.0, 80.0), Color::BLACK, 12.0, 24.0);
        ctx.pop_offset();
        let r = ctx.rects.last().expect("shadow rect emitted");
        assert_eq!((r.x, r.y), (110.0, 70.0));
        assert_eq!(r.blur, 24.0);
        assert_eq!(r.radius, 12.0);
        assert_eq!(r.border_width, 0.0);
    }

    #[test]
    fn fill_shadow_is_a_noop_without_blur() {
        // A "shadow" with no blur is just a rect; emitting a solid fill from
        // fill_shadow would surprise callers, so it draws nothing.
        let mut ctx = PaintContext::default();
        ctx.fill_shadow(Rect::new(0.0, 0.0, 10.0, 10.0), Color::BLACK, 4.0, 0.0);
        assert!(ctx.rects.is_empty(), "blur <= 0 must not emit a rect");
    }

    #[test]
    fn fill_rect_leaves_blur_zero() {
        // The crisp fill path must keep blur at 0 so the renderer's fast path
        // (and the pre-shadow geometry) is bit-for-bit unchanged.
        let mut ctx = PaintContext::default();
        ctx.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        assert_eq!(ctx.rects.last().unwrap().blur, 0.0);
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

    #[test]
    fn snap_device_px_lands_on_the_physical_grid_at_fractional_scale() {
        // The caret's leading edge is snapped through this so its ~0.5px
        // antialiased rect edge doesn't straddle two physical columns and smear
        // onto the glyph to its left. The proof the caller relies on: the
        // returned logical value, scaled back up, is a whole physical pixel —
        // at 125% / 150% just as much as at integer scales.
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let mut ctx = PaintContext::default();
            ctx.text_engine.set_scale(scale);
            for raw in [10.3_f32, 7.61, 0.49, 123.02, 42.5] {
                let snapped = ctx.snap_device_px(raw);
                let phys = snapped * scale;
                assert!(
                    (phys - phys.round()).abs() < 1e-3,
                    "scale {scale}: snap({raw}) = {snapped} lands at {phys} phys px, not on the grid"
                );
                // A snap, not a jump: it never moves the coordinate by a whole
                // logical pixel or more (that would drop the caret onto the
                // wrong glyph boundary).
                assert!(
                    (snapped - raw).abs() < 1.0,
                    "scale {scale}: snap({raw}) moved to {snapped}, more than a pixel"
                );
            }
        }
    }

    #[test]
    fn snap_device_px_rounds_to_whole_logical_px_at_scale_one() {
        // At 100% the physical and logical grids coincide, so snapping is just
        // rounding to the nearest whole logical pixel.
        let mut ctx = PaintContext::default();
        ctx.text_engine.set_scale(1.0);
        assert_eq!(ctx.snap_device_px(10.3), 10.0);
        assert_eq!(ctx.snap_device_px(10.7), 11.0);
    }

    #[test]
    fn glyph_origin_snaps_to_the_device_grid_when_upright() {
        // A `Text`/`Button` label whose flex-centered layout origin lands on a
        // fractional pixel (common even at 100% DPI) must have its crisp,
        // device-resolution bitmap placed on a whole device pixel, or it smears
        // uniformly — the "everything but Input is soft" report. The advances
        // are already whole device pixels, so snapping the origin is all it
        // takes.
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let mut ctx = PaintContext::default();
            ctx.text_engine.set_scale(scale);
            for raw in [(12.4_f32, 7.6_f32), (100.37, 42.5), (0.49, 255.9)] {
                let (sx, sy) = ctx.snap_glyph_origin(raw.0, raw.1, false);
                for (label, v) in [("x", sx), ("y", sy)] {
                    let phys = v * scale;
                    assert!(
                        (phys - phys.round()).abs() < 1e-3,
                        "scale {scale}: snapped {label} {v} = {phys} phys px, off the grid",
                    );
                }
            }
        }
    }

    #[test]
    fn a_rect_outside_the_clip_is_dropped() {
        // The renderer would scissor it to nothing; recording it only costs a
        // vertex and a batch entry. This is what bounds the draw list for
        // content taller than its ScrollView.
        let mut ctx = PaintContext::default();
        ctx.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
        ctx.fill_rect(Rect::new(0.0, -50.0, 100.0, 20.0), Color::WHITE);
        assert!(ctx.rects.is_empty(), "fully-above rect must be dropped");
        ctx.fill_rect(Rect::new(0.0, -10.0, 100.0, 20.0), Color::WHITE);
        assert_eq!(ctx.rects.len(), 1, "a straddling rect must be kept");
        ctx.pop_clip();
    }

    #[test]
    fn culling_is_off_without_a_clip() {
        // Unclipped draws (the main tree, an overlay layer) are untouched —
        // there is nothing to be outside of.
        let mut ctx = PaintContext::default();
        ctx.fill_rect(Rect::new(-1000.0, -1000.0, 10.0, 10.0), Color::WHITE);
        assert_eq!(ctx.rects.len(), 1);
    }

    #[test]
    fn a_degenerate_quad_inside_the_clip_survives() {
        // Zero-area draws — a space's empty glyph bitmap, a collapsed
        // separator — must behave exactly as before culling: the test is
        // strict disjointness, not an empty intersection.
        let mut ctx = PaintContext::default();
        ctx.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
        ctx.fill_rect(Rect::new(50.0, 50.0, 0.0, 0.0), Color::WHITE);
        assert_eq!(ctx.rects.len(), 1);
        ctx.pop_clip();
    }

    #[test]
    fn a_shadow_halo_reaching_into_the_clip_survives() {
        // A shadow's quad is inflated by `blur` on every side (the renderer
        // does the same), so the casting box can sit outside the clip while
        // the halo still shows. Culling on the un-inflated box would erase it.
        let mut ctx = PaintContext::default();
        ctx.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
        // Box ends 10px above the clip; a 24px blur reaches 14px inside it.
        ctx.fill_shadow(Rect::new(0.0, -60.0, 100.0, 50.0), Color::BLACK, 0.0, 24.0);
        assert_eq!(ctx.rects.len(), 1, "the halo overlaps the clip");
        ctx.rects.clear();
        // Same box, no reach: the blur is too small to cross the edge.
        ctx.fill_shadow(Rect::new(0.0, -60.0, 100.0, 50.0), Color::BLACK, 0.0, 4.0);
        assert!(ctx.rects.is_empty());
        ctx.pop_clip();
    }

    #[test]
    fn a_rotated_glyph_is_never_culled() {
        // Rotated glyphs are placed by rotation math about a pivot, so the
        // axis-aligned box reconstructed from the bitmap's bearings does not
        // describe where the quad lands — culling on it could erase a visible
        // chevron. `glyph_clipped_out` opts them out wholesale.
        let ctx = {
            let mut c = PaintContext::default();
            c.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            c
        };
        let image = GlyphImage {
            data: Vec::new(),
            width: 8,
            height: 8,
            left: 0,
            top: 8,
            is_color: false,
        };
        assert!(
            ctx.glyph_clipped_out(0.0, -500.0, &image, false),
            "an upright glyph far above the clip is culled"
        );
        assert!(
            !ctx.glyph_clipped_out(0.0, -500.0, &image, true),
            "the same glyph under a rotation is kept"
        );
    }

    #[test]
    fn rotated_glyph_origin_is_left_unsnapped() {
        // Icon chevrons are positioned by rotation math about a pivot; snapping
        // their origin would fight that geometry, so a rotated glyph keeps its
        // exact composited position.
        let mut ctx = PaintContext::default();
        ctx.text_engine.set_scale(1.25);
        let (x, y) = ctx.snap_glyph_origin(12.4, 7.6, true);
        assert_eq!((x, y), (12.4, 7.6));
    }
}
