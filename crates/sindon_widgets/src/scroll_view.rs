//! ScrollView — clips its children and translates them by a scroll offset.
//!
//! The widget clips children to its own bounds and shifts them vertically
//! by `scroll_y`. Mouse wheel events whose cursor is over the viewport
//! update `scroll_y`, clamped against the user-declared `content_height`.
//!
//! Phase 11 MVP: vertical scrolling only; content height is supplied by the
//! caller via [`ScrollView::content_height`]. An optional scrollbar is drawn
//! on the right edge; it is also draggable — pressing the thumb scrubs the
//! offset, and pressing the track jumps the thumb to the cursor (the same
//! pointer-capture drag [`Slider`](crate::Slider) uses).

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use sindon_core::{Color, Lerp, Rect};
use sindon_layout::FlexStyle;
use sindon_reactive::{Animated, Easing};
use std::time::Duration;

/// A scrollable viewport.
///
/// # Example (conceptual)
/// ```
/// # use sindon_widgets::ScrollView;
/// let sv = ScrollView::new()
///     .height(300.0)
///     .width_full()
///     .content_height(1200.0);
/// ```
/// Default duration of the smooth-scroll glide applied to wheel input.
/// Short enough to feel responsive, long enough to read as motion (matches
/// the hover-fade default). `scroll_transition(Duration::ZERO)` opts back out
/// to the pre-animation instant jump.
const SCROLL_TRANSITION_DEFAULT: Duration = Duration::from_millis(120);

pub struct ScrollView {
    /// Wheel scrolling retargets this animator and the displayed offset eases
    /// toward it, so a fast flick doesn't teleport the content (FW-7). Lazily
    /// created on first scroll (like `Container`'s hover fade); `None` reads as
    /// a resting offset of 0. The *target* is the logical scroll position
    /// ([`scroll_y`](Self::scroll_y)); the *current eased* value is what paint
    /// and hit-testing use ([`scroll_offset`](Widget::scroll_offset)).
    scroll_anim: Option<Animated<f32>>,
    /// Glide duration for wheel scrolling. `Duration::ZERO` = instant.
    scroll_transition: Duration,
    /// The eased offset as of the last [`Self::take_scroll_moved`] call. Lets
    /// the tree notice content sliding under a stationary cursor (a wheel
    /// glide) so it can replay the hover hit-test — hover is otherwise only
    /// ever recomputed from a live `MouseMove`. Starts at the resting 0.
    last_seen_displayed: f32,
    /// Caller-pinned content height. `None` means "auto" — the widget tree
    /// writes the laid-out children's max bottom into [`Self::auto_content_height`]
    /// after each layout pass and that value is used for scroll clamp /
    /// scrollbar.
    explicit_content_height: Option<f32>,
    /// Measured total content extent in pixels, populated by the tree after
    /// every layout pass (see `WidgetTree::sync_scroll_view_content_heights`).
    /// Includes the ScrollView's top + bottom base padding so a fully scrolled
    /// viewport shows the bottom padding flush with the last child. Ignored
    /// when [`Self::explicit_content_height`] is `Some`.
    auto_content_height: f32,
    /// A scrollbar-thumb drag in flight. `Some(grab_dy)` holds the offset
    /// between the cursor and the thumb's top edge at the moment of the press,
    /// so the thumb stays anchored under the grab point as the pointer moves
    /// (rather than snapping its top to the cursor). `None` = not dragging.
    /// Independent of hover so a captured drag survives the cursor leaving the
    /// track — mirrors [`Slider`](crate::Slider)'s `dragging` flag.
    dragging: Option<f32>,
    /// The cursor is over the (widened) thumb grab area. Purely a paint cue —
    /// the thumb brightens on hover. Recomputed each `MouseMove`; cleared on
    /// `MouseLeave`. Every pointer move already forces a repaint, so this
    /// needs no explicit redraw request.
    thumb_hovered: bool,
    style: FlexStyle,
    show_scrollbar: bool,
    /// User-supplied uniform padding (set via [`Self::padding`]). Stored
    /// separately so [`Widget::style`] can re-emit padding with the scrollbar
    /// gutter added to the right side without needing to read padding back
    /// out of Taffy's opaque `LengthPercentage`.
    base_padding: f32,
    // Colors (None = theme fallback)
    background: Option<Color>,
    track_color: Option<Color>,
    thumb_color: Option<Color>,
}

impl ScrollView {
    /// Create a new scroll view (column-oriented content).
    pub fn new() -> Self {
        Self {
            scroll_anim: None,
            scroll_transition: SCROLL_TRANSITION_DEFAULT,
            last_seen_displayed: 0.0,
            explicit_content_height: None,
            auto_content_height: 0.0,
            dragging: None,
            thumb_hovered: false,
            // A scroll container must declare `overflow: hidden` so flex
            // layout lets it shrink below its content. By default a flex item's
            // automatic minimum size is its content size, so a `grow(1.0)`
            // viewport nested under grow containers (no explicit height) would
            // balloon to its overflowing content instead of clamping to the
            // allocated space — leaving nothing to scroll. `overflow: hidden`
            // sets that automatic minimum to 0 *and* stops the overflow from
            // contributing to ancestors' scroll regions, so the intermediate
            // `grow` containers don't balloon either. Visual clipping is done
            // in paint; this only affects layout, and `Hidden` reserves no
            // scrollbar gutter (we draw our own).
            style: FlexStyle::new().column().overflow_hidden(),
            show_scrollbar: true,
            base_padding: 0.0,
            background: None,
            track_color: None,
            thumb_color: None,
        }
    }

    /// Pin the total height of the scrollable content.
    ///
    /// By default a `ScrollView` measures its laid-out children every layout
    /// pass and uses the max bottom as `content_height` — call this only when
    /// you need to lock the value (e.g. virtualized lists where the rendered
    /// subtree is shorter than the logical content).
    pub fn content_height(mut self, h: f32) -> Self {
        self.explicit_content_height = Some(h);
        self
    }

    /// Pin the content extent at runtime. Used by
    /// [`VirtualList`](crate::VirtualList), which knows the full logical height
    /// (item count × row height) even though only a window of rows is
    /// materialized, so the auto-measured height (a screenful) would be wrong.
    /// Wins over the auto-measured value, same as the [`Self::content_height`]
    /// builder.
    pub(crate) fn set_pinned_content_height(&mut self, h: f32) {
        self.explicit_content_height = Some(h);
    }

    /// Effective content extent — explicit value if set, otherwise the value
    /// the tree wrote after the last layout pass.
    fn effective_content_height(&self) -> f32 {
        self.explicit_content_height
            .unwrap_or(self.auto_content_height)
    }

    /// Tree-side hook (same crate) for writing the measured content extent.
    /// Always updates `auto_content_height`; a caller's explicit
    /// [`Self::content_height`] still wins via [`Self::effective_content_height`].
    pub(crate) fn set_measured_content_height(&mut self, h: f32) {
        self.auto_content_height = h;
    }

    /// Padding the tree should add to the children's measured bottom to
    /// produce the total content extent (matches the top padding that Taffy
    /// has already baked into each child's relative y).
    pub(crate) fn measured_bottom_padding(&self) -> f32 {
        self.base_padding
    }

    /// Set the fixed viewport height.
    pub fn height(mut self, px: f32) -> Self {
        self.style = self.style.height(px);
        self
    }

    /// Set the fixed viewport width.
    pub fn width(mut self, px: f32) -> Self {
        self.style = self.style.width(px);
        self
    }

    /// Fill available width.
    pub fn width_full(mut self) -> Self {
        self.style = self.style.width_full();
        self
    }

    /// Fill available height.
    pub fn height_full(mut self) -> Self {
        self.style = self.style.height_full();
        self
    }

    /// Flex-grow factor — the scroll view claims this share of any leftover
    /// space along the parent's main axis. Pair with `.grow(1.0)` siblings
    /// for the common "viewport fills whatever the header leaves" layout.
    pub fn grow(mut self, factor: f32) -> Self {
        self.style = self.style.grow(factor);
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, px: f32) -> Self {
        self.style = self.style.gap(px);
        self
    }

    /// Set uniform padding on the viewport. The right side may receive
    /// additional padding for the scrollbar gutter — see [`Widget::style`].
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self.base_padding = px;
        self
    }

    /// Toggle the visual scrollbar (drawn on the right edge).
    pub fn show_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }

    /// Override the viewport background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Override the scrollbar track color.
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = Some(color);
        self
    }

    /// Override the scrollbar thumb color.
    pub fn thumb_color(mut self, color: Color) -> Self {
        self.thumb_color = Some(color);
        self
    }

    /// Set how long a wheel scroll takes to glide to its new position.
    /// Defaults to `SCROLL_TRANSITION_DEFAULT` (120 ms); pass
    /// [`Duration::ZERO`] for the pre-animation instant jump (FW-7).
    pub fn scroll_transition(mut self, duration: Duration) -> Self {
        self.scroll_transition = duration;
        self
    }

    /// The logical (target) vertical scroll offset in pixels — where the
    /// content is heading. Equals the displayed offset once any glide settles.
    /// Wheel input and clamping operate on this; the smoothly-eased value the
    /// user sees is [`scroll_offset`](Widget::scroll_offset).
    pub fn scroll_y(&self) -> f32 {
        self.scroll_anim.as_ref().map_or(0.0, |a| a.target())
    }

    /// The current eased offset the content is drawn at — lags the target
    /// while a wheel glide is in flight, then matches it. Reads also vote for
    /// another frame while animating (see [`Animated`]).
    fn displayed_scroll(&self) -> f32 {
        self.scroll_anim.as_ref().map_or(0.0, |a| a.get())
    }

    /// Retarget the scroll glide, lazily creating the animator with the
    /// configured [`scroll_transition`](Self::scroll_transition) duration.
    fn drive_scroll(&mut self, to: f32) {
        self.scroll_anim
            .get_or_insert_with(|| Animated::new(0.0, self.scroll_transition, Easing::EaseOut))
            .set(to);
    }

    /// Set the scroll offset *instantly* — displayed and target both jump to
    /// `to` with no glide. Wheel scrolling eases (see [`Self::drive_scroll`]),
    /// but a scrollbar-thumb drag must track the pointer 1:1, so it snaps: an
    /// eased glide would leave the thumb lagging behind the cursor.
    fn snap_scroll(&mut self, to: f32) {
        self.scroll_anim
            .get_or_insert_with(|| Animated::new(0.0, self.scroll_transition, Easing::EaseOut))
            .snap(to);
    }

    /// Maximum allowed scroll offset given the current viewport height.
    pub fn max_scroll_y(&self, viewport_height: f32) -> f32 {
        (self.effective_content_height() - viewport_height).max(0.0)
    }

    /// Scroll the minimum distance that brings the vertical span
    /// `[top, top + height]` inside the viewport, and return the resulting
    /// scroll target.
    ///
    /// `top` is measured in *content* space — the frame the children's layout
    /// rects live in, before [`Widget::scroll_offset`] shifts them — so a
    /// caller converts a descendant's rect with `descendant.y - viewport.y`.
    ///
    /// A span that is already fully visible does not move: the current target
    /// is only clamped into the range of offsets that satisfy both edges. When
    /// the span is taller than the viewport that range inverts (every offset
    /// between the bounds leaves it covering the viewport entirely), which the
    /// same clamp handles once the bounds are ordered — a covering span stays
    /// put, and one that is off-screen is pulled in by its nearest edge.
    ///
    /// Retargets the same glide wheel scrolling uses: this is a response to a
    /// user gesture (Tab), so the content slides rather than teleports.
    pub(crate) fn reveal_span(&mut self, top: f32, height: f32, viewport_height: f32) -> f32 {
        let cur = self.scroll_y();
        // A zero-height viewport means the tree has not laid this view out
        // yet; every bound below would be nonsense, so keep the offset.
        if viewport_height <= 0.0 {
            return cur;
        }
        // The offsets that put the span's top edge at the viewport top, and
        // its bottom edge at the viewport bottom, respectively.
        let at_top = top;
        let at_bottom = top + height - viewport_height;
        let (lo, hi) = if at_bottom <= at_top {
            (at_bottom, at_top)
        } else {
            (at_top, at_bottom)
        };
        let new = cur
            .clamp(lo, hi)
            .clamp(0.0, self.max_scroll_y(viewport_height));
        if new != cur {
            self.drive_scroll(new);
        }
        new
    }

    /// Re-clamp the scroll offset to the current content/viewport. The tree
    /// calls this after each layout pass (see
    /// `WidgetTree::sync_scroll_view_content_heights`) so that when the content
    /// shrinks — e.g. switching to a shorter note, or deleting text — a stale
    /// offset doesn't leave the top scrolled out of view. The offset only ever
    /// moves *down* to the new maximum (0 when nothing overflows); growing
    /// content leaves it untouched, so an active scroll position is preserved.
    pub(crate) fn clamp_scroll(&mut self, viewport_height: f32) {
        let max = self.max_scroll_y(viewport_height);
        if let Some(a) = &self.scroll_anim
            && a.target() > max
        {
            // Re-clamp is a system correction, not a user gesture — snap
            // instantly so switching to a shorter note doesn't visibly
            // slide the (reused) viewport back up.
            a.snap(max);
        }
    }

    /// Whether the eased offset moved since the previous call, latching the
    /// new value. The tree polls this once per layout pass (see
    /// `WidgetTree::sync_scroll_view_content_heights`): a wheel glide slides
    /// the content under a stationary cursor with no `MouseMove` to refresh
    /// hover, so a move here asks the tree to replay the hover hit-test — the
    /// same "geometry shifted under a still cursor" case a layer pop or a
    /// self-rebuild already handle.
    ///
    /// Comparing the displayed delta (rather than the animator's
    /// `is_animating`) catches the settling frame too — the one where the
    /// offset lands exactly on its target, which reads as already settled —
    /// and falls silent the instant the content comes to rest, so a view at
    /// anchor reports nothing and adds no per-frame work. Call it after
    /// [`Self::clamp_scroll`] so a re-clamp snap counts as the move it is.
    pub(crate) fn take_scroll_moved(&mut self) -> bool {
        let cur = self.displayed_scroll();
        let moved = cur != self.last_seen_displayed;
        self.last_seen_displayed = cur;
        moved
    }

    /// Scrollbar track + thumb geometry for the given viewport `layout`, or
    /// `None` when the scrollbar is hidden or nothing overflows (so there is
    /// no bar to draw or grab). The single source of truth shared by paint and
    /// drag hit-testing — deriving both from here keeps what the user sees and
    /// what they can grab exactly aligned, the same paint/hit-test contract the
    /// caret and inner scroll hold. The thumb is positioned from the *displayed*
    /// (eased) offset, so it glides with the content.
    fn scrollbar_geom(&self, layout: Rect) -> Option<ScrollbarGeom> {
        if !self.show_scrollbar {
            return None;
        }
        let viewport_h = layout.size.height;
        let content_h = self.effective_content_height();
        if content_h <= viewport_h || viewport_h <= 0.0 {
            return None;
        }

        let track_x = layout.origin.x + layout.size.width - SCROLLBAR_WIDTH - SCROLLBAR_INSET;
        let track_rect = Rect::new(track_x, layout.origin.y, SCROLLBAR_WIDTH, viewport_h);

        let thumb_h = ((viewport_h / content_h) * viewport_h)
            .max(SCROLLBAR_THUMB_MIN)
            .min(viewport_h);
        let max_scroll = (content_h - viewport_h).max(0.0);
        let travel = viewport_h - thumb_h;
        let progress = if max_scroll > 0.0 {
            (self.displayed_scroll() / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = layout.origin.y + progress * travel;
        let thumb_rect = Rect::new(track_x, thumb_y, SCROLLBAR_WIDTH, thumb_h);

        Some(ScrollbarGeom {
            track_rect,
            thumb_rect,
            travel,
            max_scroll,
        })
    }

    /// Left edge of the pointer grab band. The visual track is only
    /// [`SCROLLBAR_WIDTH`] wide — too thin to reliably hit with a mouse — so the
    /// interactive region spans the whole reserved [`SCROLLBAR_GUTTER`]. Children
    /// are laid out inside that gutter's padding, so widening the grab area here
    /// never steals a click from them.
    fn grab_band_left(layout: Rect) -> f32 {
        layout.right() - SCROLLBAR_GUTTER
    }

    /// Map a cursor y (viewport space) to a scroll offset and snap to it, so the
    /// thumb's top edge lands at `cursor_y - grab_dy`. The 1:1 tracking path for
    /// a thumb drag; a no-op when there is no room to scroll.
    fn drag_to(&mut self, cursor_y: f32, grab_dy: f32, geom: &ScrollbarGeom, layout: Rect) {
        if geom.travel <= 0.0 || geom.max_scroll <= 0.0 {
            return;
        }
        let thumb_top = cursor_y - grab_dy;
        let progress = ((thumb_top - layout.origin.y) / geom.travel).clamp(0.0, 1.0);
        self.snap_scroll(progress * geom.max_scroll);
    }
}

/// Scrollbar track + thumb rects plus the two scalars a drag needs, returned by
/// [`ScrollView::scrollbar_geom`]. `travel` is the vertical range the thumb top
/// spans (`viewport_h − thumb_h`); `max_scroll` is the matching content offset
/// range, so `offset = (thumb_top / travel) · max_scroll`.
struct ScrollbarGeom {
    track_rect: Rect,
    thumb_rect: Rect,
    travel: f32,
    max_scroll: f32,
}

impl Default for ScrollView {
    fn default() -> Self {
        Self::new()
    }
}

/// Width of the visual scrollbar in pixels.
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Inset from the right edge.
const SCROLLBAR_INSET: f32 = 2.0;
/// Minimum height of the scrollbar thumb.
const SCROLLBAR_THUMB_MIN: f32 = 16.0;
/// Total horizontal space reserved on the right for the scrollbar.
///
/// Equals scrollbar width + edge inset + a small breathing margin so children
/// do not visually crowd the track. The reservation is unconditional whenever
/// `show_scrollbar` is true (cf. CSS `scrollbar-gutter: stable`) — it avoids
/// horizontal reflow at the moment content starts overflowing and is the fix
/// for the bug where unbreakable strings (e.g. long `AAAAAA…`) drew under the
/// scrollbar track.
const SCROLLBAR_GUTTER: f32 = SCROLLBAR_WIDTH + SCROLLBAR_INSET + 4.0;

impl Widget for ScrollView {
    /// Returns the layout style with the scrollbar gutter folded into the
    /// right padding when [`Self::show_scrollbar`] is on. Children laid out
    /// inside this padded box never overlap the scrollbar track, even when
    /// they declare `width_full` and the content overflows. Cleared by
    /// `show_scrollbar(false)`.
    fn style(&self) -> FlexStyle {
        if self.show_scrollbar {
            self.style.clone().padding_trbl(
                self.base_padding,
                self.base_padding + SCROLLBAR_GUTTER,
                self.base_padding,
                self.base_padding,
            )
        } else {
            self.style.clone()
        }
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        // Viewport background (drawn without any scroll offset).
        let bg = self.background.unwrap_or(ctx.theme.colors.surface);
        ctx.fill_rect(layout, bg);
    }

    fn paint_pre_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.push_clip(layout);
        // Snap the *composited* scroll offset to the device grid before pushing
        // it. Glyphs re-snap their own origin (`snap_glyph_origin`), but the row
        // backgrounds and divider rects that ride this offset do not — so during
        // a smooth-scroll glide an un-snapped fractional offset lands their edges
        // between physical columns and they shimmer. Fold in any active layer
        // offset, snap the composite (so we sit on the same grid the renderer
        // rasterizes against), then push the delta. Hit-testing keeps the raw
        // fractional offset (see `scroll_offset`) — the same paint-snaps /
        // hit-test-doesn't contract as the caret and the editor's inner scroll.
        let (_, oy) = ctx.current_offset();
        let snapped = ctx.snap_device_px(oy - self.displayed_scroll()) - oy;
        ctx.push_offset(0.0, snapped);
    }

    fn paint_post_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.pop_offset();
        ctx.pop_clip();

        // Scrollbar (drawn on top, unclipped, at viewport coords).
        let Some(geom) = self.scrollbar_geom(layout) else {
            return;
        };

        let track_color = self.track_color.unwrap_or(ctx.theme.colors.surface_variant);
        let mut thumb_color = self
            .thumb_color
            .unwrap_or(ctx.theme.colors.on_surface_variant);
        // Brighten the thumb toward the foreground while grabbed, and a little
        // on hover, so the bar reads as a live control rather than a passive
        // indicator. A drag owns the pointer, so its cue outranks hover.
        if self.dragging.is_some() {
            thumb_color = thumb_color.lerp(&ctx.theme.colors.on_surface, 0.55);
        } else if self.thumb_hovered {
            thumb_color = thumb_color.lerp(&ctx.theme.colors.on_surface, 0.3);
        }

        ctx.fill_rect(geom.track_rect, track_color);
        ctx.fill_rect(geom.thumb_rect, thumb_color);
    }

    fn scroll_offset(&self) -> (f32, f32) {
        // Hit-testing must use the displayed (eased) offset so a click lands on
        // the glyph the user currently sees mid-glide, matching paint.
        (0.0, self.displayed_scroll())
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::Scroll {
                position, delta_y, ..
            } => {
                if !layout.contains(*position) {
                    return EventResult::Ignored;
                }
                let max = self.max_scroll_y(layout.size.height);
                // Accumulate against the *target*, not the in-flight displayed
                // value, so consecutive wheel ticks add up instead of fighting
                // the glide. `drive_scroll` eases the displayed offset toward it.
                let new_y = (self.scroll_y() - delta_y).clamp(0.0, max);
                if new_y != self.scroll_y() {
                    self.drive_scroll(new_y);
                }
                EventResult::Consumed
            }

            // A press in the scrollbar's grab band begins a drag; children are
            // dispatched before this widget and never occupy the gutter, so a
            // press anywhere else falls through (Ignored) exactly as before.
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                position,
            } => {
                let Some(geom) = self.scrollbar_geom(layout) else {
                    return EventResult::Ignored;
                };
                if position.x < Self::grab_band_left(layout) {
                    return EventResult::Ignored;
                }
                // Pressing the thumb keeps its grab point under the cursor;
                // pressing the bare track jumps the thumb *centre* to the cursor
                // (then drags anchored there).
                let on_thumb = position.y >= geom.thumb_rect.origin.y
                    && position.y <= geom.thumb_rect.bottom();
                let grab_dy = if on_thumb {
                    position.y - geom.thumb_rect.origin.y
                } else {
                    geom.thumb_rect.size.height / 2.0
                };
                self.dragging = Some(grab_dy);
                self.thumb_hovered = true;
                ctx.capture_pointer();
                self.drag_to(position.y, grab_dy, &geom, layout);
                EventResult::Consumed
            }

            WidgetEvent::MouseMove { position } => {
                if let Some(grab_dy) = self.dragging {
                    if let Some(geom) = self.scrollbar_geom(layout) {
                        self.drag_to(position.y, grab_dy, &geom, layout);
                    }
                    EventResult::Consumed
                } else {
                    // Hover cue only — do not consume, so normal hover routing
                    // over the content is undisturbed. The repaint every move
                    // already triggers picks up the colour change.
                    self.thumb_hovered = self
                        .scrollbar_geom(layout)
                        .map(|g| {
                            position.x >= Self::grab_band_left(layout)
                                && position.y >= g.thumb_rect.origin.y
                                && position.y <= g.thumb_rect.bottom()
                        })
                        .unwrap_or(false);
                    EventResult::Ignored
                }
            }

            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.dragging.take().is_some() {
                    ctx.release_pointer();
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }

            // Cursor left the viewport: drop the hover cue. A drag in flight
            // owns a captured pointer and is ended by its MouseUp, not by this.
            WidgetEvent::MouseLeave => {
                self.thumb_hovered = false;
                EventResult::Ignored
            }

            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sindon_core::Point;

    /// 1000px of content in a 200px viewport, so there is room to scroll in
    /// both directions from any offset the tests park at.
    fn view(scroll: f32) -> ScrollView {
        let mut sv = ScrollView::new().content_height(1000.0);
        sv.drive_scroll(scroll);
        sv
    }

    #[test]
    fn a_visible_span_is_left_alone() {
        // Sitting at 300, the viewport shows content 300..500. A span inside
        // that is already visible: revealing it must not move anything.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(350.0, 40.0, 200.0), 300.0);
        assert_eq!(sv.scroll_y(), 300.0);
    }

    #[test]
    fn a_span_below_the_viewport_scrolls_its_bottom_edge_into_view() {
        // Span at 520..560 is past the bottom (viewport ends at 500). The
        // minimum move puts its bottom edge on the viewport bottom, which
        // leaves it *just* visible rather than jumping it to the top.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(520.0, 40.0, 200.0), 360.0);
        assert_eq!(sv.scroll_y(), 360.0);
    }

    #[test]
    fn a_span_above_the_viewport_scrolls_its_top_edge_into_view() {
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(100.0, 40.0, 200.0), 100.0);
        assert_eq!(sv.scroll_y(), 100.0);
    }

    #[test]
    fn revealing_the_first_span_reaches_the_very_top() {
        // The top edge of the content must be reachable — the case where Tab
        // wraps back around to the first widget.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(0.0, 40.0, 200.0), 0.0);
    }

    #[test]
    fn the_reveal_never_scrolls_past_the_content_bounds() {
        // A span at the very end must not scroll past max (content - viewport
        // = 800), and one that would demand a negative offset clamps at 0.
        let mut sv = view(0.0);
        assert_eq!(sv.reveal_span(960.0, 40.0, 200.0), 800.0);

        let mut sv = view(500.0);
        // A span taller than the whole content can't be satisfied at the
        // bottom; the clamp keeps the offset legal.
        assert_eq!(sv.reveal_span(0.0, 1000.0, 200.0), 500.0);
    }

    #[test]
    fn a_span_taller_than_the_viewport_stays_put_while_it_covers() {
        // 400px span at 200..600 with the viewport at 300..500: the span
        // already fills the viewport, so there is no "more visible" to get to
        // and the offset must not move.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 300.0);
    }

    #[test]
    fn a_span_taller_than_the_viewport_is_pulled_in_by_its_nearest_edge() {
        // Same 400px span, but the viewport is above it (0..200). Aligning its
        // top is the nearest way in.
        let mut sv = view(0.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 200.0);

        // ...and from below (viewport 700..900), its bottom.
        let mut sv = view(700.0);
        assert_eq!(sv.reveal_span(200.0, 400.0, 200.0), 400.0);
    }

    #[test]
    fn an_unlaid_out_viewport_is_not_scrolled() {
        // Focus can be applied before the first layout pass; a zero-height
        // viewport must be treated as "no information", not as a reason to
        // reset the offset.
        let mut sv = view(300.0);
        assert_eq!(sv.reveal_span(500.0, 40.0, 0.0), 300.0);
        assert_eq!(sv.scroll_y(), 300.0);
    }

    /// Park the eased offset at an exact fractional value, timing-independently:
    /// a zero-duration `Animated` rests settled at its initial, so `get()`
    /// (hence `displayed_scroll`) returns it verbatim.
    fn view_displayed_at(offset: f32) -> ScrollView {
        let mut sv = ScrollView::new().content_height(1000.0);
        sv.scroll_anim = Some(Animated::new(offset, Duration::ZERO, Easing::EaseOut));
        sv
    }

    #[test]
    fn paint_snaps_the_scroll_offset_to_the_device_grid() {
        // The eased offset the content is translated by must land on the
        // physical grid, or the row backgrounds / dividers that ride it (unlike
        // glyphs, which re-snap their own origin) shimmer mid-glide. The proof:
        // the composited offset `paint_pre_children` pushes, scaled to physical
        // pixels, is a whole pixel — at 125% / 150% just as at integer scales.
        let disp = 30.37_f32;
        let layout = Rect::new(0.0, 0.0, 100.0, 200.0);
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let sv = view_displayed_at(disp);
            let mut ctx = PaintContext::default();
            ctx.text_engine.set_scale(scale);
            sv.paint_pre_children(layout, &mut ctx);

            let (_, oy) = ctx.current_offset();
            let phys = oy * scale;
            assert!(
                (phys - phys.round()).abs() < 1e-3,
                "scale {scale}: pushed offset {oy} = {phys} phys px, off the device grid"
            );
            // A snap, not a jump: never off the true offset by a whole logical
            // pixel, or the content would slide visibly out from under the user.
            assert!(
                (oy - (-disp)).abs() < 1.0,
                "scale {scale}: pushed offset {oy} moved more than a pixel from {}",
                -disp
            );
        }
    }

    #[test]
    fn hit_test_offset_keeps_the_raw_fractional_value() {
        // Paint snaps; hit-testing must not — a click mid-glide has to land on
        // the row the user sees under the cursor, so `scroll_offset` reports the
        // exact eased value with no device-grid rounding.
        let disp = 30.37_f32;
        let sv = view_displayed_at(disp);
        assert_eq!(sv.scroll_offset(), (0.0, disp));
    }

    // ── Scrollbar drag ──────────────────────────────────────────────────
    //
    // A 100×200 viewport over 1000px of content: thumb_h = (200/1000)·200 = 40,
    // travel = 160, max_scroll = 800, so a thumb-top of `y` maps to offset
    // `(y/160)·800 = 5y`. The grab band starts at right() − GUTTER = 88, and the
    // resting thumb (offset 0) occupies y ∈ [0, 40].
    fn drag_layout() -> Rect {
        Rect::new(0.0, 0.0, 100.0, 200.0)
    }

    fn down(x: f32, y: f32) -> WidgetEvent {
        WidgetEvent::MouseDown {
            position: Point::new(x, y),
            button: MouseButton::Left,
        }
    }

    fn mv(x: f32, y: f32) -> WidgetEvent {
        WidgetEvent::MouseMove {
            position: Point::new(x, y),
        }
    }

    #[test]
    fn scrollbar_geom_matches_the_documented_layout() {
        let sv = ScrollView::new().content_height(1000.0);
        let g = sv
            .scrollbar_geom(drag_layout())
            .expect("content overflows, so there is a bar");
        assert_eq!(g.thumb_rect.size.height, 40.0);
        assert_eq!(g.travel, 160.0);
        assert_eq!(g.max_scroll, 800.0);
        // Track sits at right − width − inset = 100 − 6 − 2.
        assert_eq!(g.track_rect.origin.x, 92.0);
    }

    #[test]
    fn no_bar_when_content_fits() {
        // Content shorter than the viewport: nothing to scroll, no geometry.
        let sv = ScrollView::new().content_height(100.0);
        assert!(sv.scrollbar_geom(drag_layout()).is_none());
    }

    #[test]
    fn dragging_the_thumb_scrubs_the_offset() {
        let mut sv = ScrollView::new().content_height(1000.0);
        let mut ctx = EventContext::new();

        // Press the very top of the thumb (grab_dy = 0): nothing moves yet.
        let r = sv.event(&down(94.0, 0.0), drag_layout(), &mut ctx);
        assert_eq!(r, EventResult::Consumed);
        assert_eq!(ctx.take_capture_change(), Some(true), "press captures");
        assert_eq!(sv.scroll_y(), 0.0);

        // Drag to the middle of the travel → half of max_scroll.
        sv.event(&mv(94.0, 80.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 400.0, "80/160 of 800");

        // Past the end clamps at max, and the offset snaps (no glide): the
        // displayed value equals the target immediately.
        sv.event(&mv(94.0, 300.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 800.0);
        assert_eq!(sv.scroll_offset(), (0.0, 800.0), "drag snaps, no lag");

        // Release ends the drag and hands the pointer back.
        let r = sv.event(
            &WidgetEvent::MouseUp {
                position: Point::new(94.0, 300.0),
                button: MouseButton::Left,
            },
            drag_layout(),
            &mut ctx,
        );
        assert_eq!(r, EventResult::Consumed);
        assert_eq!(ctx.take_capture_change(), Some(false), "release lets go");
        assert!(sv.dragging.is_none());
    }

    #[test]
    fn the_grab_point_is_anchored_under_the_cursor() {
        // Pressing partway down the thumb must not teleport it: the same spot on
        // the thumb stays under the cursor. Grab at thumb-y 30 (grab_dy = 30),
        // then a move to cursor-y 110 puts the thumb top at 80 → offset 400.
        let mut sv = ScrollView::new().content_height(1000.0);
        let mut ctx = EventContext::new();
        sv.event(&down(94.0, 30.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 0.0, "pressing the thumb does not jump it");
        sv.event(&mv(94.0, 110.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 400.0);
    }

    #[test]
    fn pressing_the_track_jumps_the_thumb_centre_to_the_cursor() {
        // A press on the bare track below the thumb centres the thumb on the
        // cursor: cursor-y 150, thumb_h/2 = 20 → thumb top 130 → offset 650.
        let mut sv = ScrollView::new().content_height(1000.0);
        let mut ctx = EventContext::new();
        sv.event(&down(94.0, 150.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 650.0);
    }

    #[test]
    fn a_press_in_the_content_area_is_left_for_the_children() {
        // Left of the grab band (x < 88) is content, not scrollbar: the press
        // must fall through untouched so a child can handle it, and no drag or
        // capture begins.
        let mut sv = ScrollView::new().content_height(1000.0);
        let mut ctx = EventContext::new();
        let r = sv.event(&down(10.0, 100.0), drag_layout(), &mut ctx);
        assert_eq!(r, EventResult::Ignored);
        assert!(sv.dragging.is_none());
        assert_eq!(ctx.take_capture_change(), None, "no capture requested");
    }

    #[test]
    fn a_press_with_nothing_to_scroll_is_ignored() {
        // No overflow → no bar → the press is not ours even inside the gutter.
        let mut sv = ScrollView::new().content_height(100.0);
        let mut ctx = EventContext::new();
        let r = sv.event(&down(94.0, 100.0), drag_layout(), &mut ctx);
        assert_eq!(r, EventResult::Ignored);
        assert!(sv.dragging.is_none());
    }

    #[test]
    fn hover_over_the_thumb_sets_the_paint_cue_without_consuming() {
        // A hover move over the thumb marks it (for the brighten) but must not
        // consume the move — content hover routing has to keep working.
        let mut sv = ScrollView::new().content_height(1000.0);
        let mut ctx = EventContext::new();

        let r = sv.event(&mv(94.0, 20.0), drag_layout(), &mut ctx);
        assert_eq!(r, EventResult::Ignored, "hover moves are never consumed");
        assert!(sv.thumb_hovered, "cursor is over the thumb");

        // Moving off the thumb (still in the viewport) clears it.
        sv.event(&mv(94.0, 150.0), drag_layout(), &mut ctx);
        assert!(!sv.thumb_hovered);

        // Re-hover, then leave the viewport entirely: the cue drops.
        sv.event(&mv(94.0, 20.0), drag_layout(), &mut ctx);
        assert!(sv.thumb_hovered);
        sv.event(&WidgetEvent::MouseLeave, drag_layout(), &mut ctx);
        assert!(!sv.thumb_hovered);
    }

    #[test]
    fn a_captured_move_past_the_track_top_clamps_at_zero() {
        // Mid-drag the pointer can wander above the viewport (capture keeps
        // delivering moves). A negative thumb-top must clamp to offset 0, not
        // scroll to a negative position. Park deterministically at offset 400
        // (thumb at y ∈ [80, 120]) so the grab is timing-independent.
        let mut sv = view_displayed_at(400.0);
        let mut ctx = EventContext::new();
        sv.event(&down(94.0, 80.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 400.0, "grabbing the thumb top holds");
        sv.event(&mv(94.0, -50.0), drag_layout(), &mut ctx);
        assert_eq!(sv.scroll_y(), 0.0);
    }
}
