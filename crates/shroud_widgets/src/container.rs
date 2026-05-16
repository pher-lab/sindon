//! Container widget — a flexbox layout container.

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::Reactive;

/// A flexbox container widget.
///
/// Containers can have a background color and arrange their children
/// in a row or column via flexbox.
///
/// The background is stored as [`Reactive<Color>`], so the setter accepts
/// either a literal `Color` or a signal-backed source (`Signal<Color>`,
/// `Memo<Color>`, `Reactive::derive(...)`). Dynamic variants are re-read
/// on every paint.
pub struct Container {
    style: FlexStyle,
    background: Option<Reactive<Color>>,
    /// Hover override. `Some` (set via [`Container::hover_background`]) wins
    /// over the theme; `None` with `hoverable == true` falls back to
    /// `theme.hover.bg`.
    hover_bg: Option<Reactive<Color>>,
    /// Whether this container reacts to pointer hover. Off by default —
    /// the vast majority of containers are passive layout boxes, so
    /// enrolling every one in MouseEnter/Leave routing would be wasted
    /// work. Flipped on by [`Container::hoverable`] or by setting an
    /// explicit hover bg.
    hoverable: bool,
    hovered: bool,
    radius: f32,
    visible: Reactive<bool>,
}

impl Container {
    /// Create a column container (vertical stacking). Cross axis is horizontal;
    /// children stretch to the column's full width by default — see
    /// [`FlexStyle::column`] for the cross-axis story.
    pub fn column() -> Self {
        Self {
            style: FlexStyle::new().column(),
            background: None,
            hover_bg: None,
            hoverable: false,
            hovered: false,
            radius: 0.0,
            visible: Reactive::Static(true),
        }
    }

    /// Create a row container (horizontal stacking). Cross axis is vertical;
    /// the default `Stretch` makes every child grow to the tallest sibling's
    /// height. For a header that mixes different-height widgets (e.g. a large
    /// title next to a button), chain [`Self::align_center`] to size each
    /// child to its own height and vertically center them — the button's
    /// label will then sit on the same visual baseline as the title.
    pub fn row() -> Self {
        Self {
            style: FlexStyle::new().row(),
            background: None,
            hover_bg: None,
            hoverable: false,
            hovered: false,
            radius: 0.0,
            visible: Reactive::Static(true),
        }
    }

    /// Toggle visibility. `false` gives `display: none` semantics — the
    /// container and its subtree are removed from the layout flow, not
    /// painted, and do not receive events.
    ///
    /// Accepts a literal `bool`, `Signal<bool>`, `Memo<bool>`, or
    /// `Reactive::derive(...)`. The reactive source is re-read every frame,
    /// so wrap expensive closures in a `Memo` if needed.
    pub fn visible(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.visible = v.into();
        self
    }

    /// Set the background color.
    ///
    /// Accepts a literal `Color`, `Signal<Color>`, `Memo<Color>`, or
    /// `Reactive::derive(...)`.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.background = Some(color.into());
        self
    }

    /// Enable pointer-hover styling using the theme's `hover.bg` token.
    ///
    /// Without this (or [`Container::hover_background`]), a container is
    /// inert to pointer enter/leave — the common case, so opt-in keeps
    /// the renderer from re-painting passive layout boxes whenever the
    /// cursor moves through them.
    ///
    /// Combine with [`Container::background`] for the "row that lifts off
    /// surface when the cursor enters" pattern (list items, menu rows).
    pub fn hoverable(mut self) -> Self {
        self.hoverable = true;
        self
    }

    /// Set an explicit background color for the hover state. Implies
    /// [`Container::hoverable`] — calling this alone is enough to opt in.
    pub fn hover_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.hover_bg = Some(color.into());
        self.hoverable = true;
        self
    }

    /// Round the corners of the background fill by `px`. No effect when no
    /// `background` is set, since rounding only applies to the painted rect.
    /// Negative values are clamped to `0.0`; values larger than half of the
    /// shorter side are clamped per-frame in the renderer (no need for the
    /// caller to know the final size).
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
        self
    }

    /// Set padding on all sides.
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, px: f32) -> Self {
        self.style = self.style.gap(px);
        self
    }

    /// Set fixed width.
    pub fn width(mut self, px: f32) -> Self {
        self.style = self.style.width(px);
        self
    }

    /// Set fixed height.
    pub fn height(mut self, px: f32) -> Self {
        self.style = self.style.height(px);
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

    /// Center children on both axes. See [`FlexStyle::center`] — note that
    /// this collapses children to min-content on the cross axis. For
    /// vertical-only centering in a column without collapsing child width,
    /// use [`Container::justify_center`] instead.
    pub fn center(mut self) -> Self {
        self.style = self.style.center();
        self
    }

    /// Center children on the main axis only. In a column container this
    /// gives vertical centering while leaving children at their natural
    /// width; pair with [`Container::max_width`] for a centered card layout.
    pub fn justify_center(mut self) -> Self {
        self.style = self.style.justify_center();
        self
    }

    /// Center children on the cross axis only. In a column container this
    /// gives horizontal centering. Note the same caveat as [`Container::center`]:
    /// children without explicit cross-axis sizing collapse to min-content,
    /// so combine with [`Container::max_width`] (which acts as a size hint
    /// when set together with `width_full`) or an explicit child width.
    pub fn align_center(mut self) -> Self {
        self.style = self.style.align_center();
        self
    }

    /// Clamp the container's width. Combined with `width_full()` this yields
    /// the common "fluid up to N px" pattern (Tailwind `max-w-md` etc.).
    pub fn max_width(mut self, px: f32) -> Self {
        self.style = self.style.max_width(px);
        self
    }

    /// Clamp the container's height.
    pub fn max_height(mut self, px: f32) -> Self {
        self.style = self.style.max_height(px);
        self
    }

    /// Grow to fill available space.
    pub fn grow(mut self, factor: f32) -> Self {
        self.style = self.style.grow(factor);
        self
    }
}

impl Widget for Container {
    fn style(&self) -> FlexStyle {
        self.style.clone()
    }

    fn visible(&self) -> bool {
        self.visible.get()
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let hover_bg = if self.hoverable && self.hovered {
            Some(
                self.hover_bg
                    .as_ref()
                    .map(|c| c.get())
                    .unwrap_or(ctx.theme.hover.bg),
            )
        } else {
            None
        };
        let bg = hover_bg.or_else(|| self.background.as_ref().map(|c| c.get()));
        if let Some(color) = bg {
            ctx.fill_rect_rounded(layout, color, self.radius);
        }
    }

    fn event(
        &mut self,
        event: &WidgetEvent,
        _layout: Rect,
        _ctx: &mut EventContext,
    ) -> EventResult {
        // Stay inert when not opted in — keeps the no-hover path identical
        // to the pre-A4 behavior (no events consumed, no extra book-keeping).
        if !self.hoverable {
            return EventResult::Ignored;
        }
        match event {
            WidgetEvent::MouseEnter => {
                self.hovered = true;
                // Don't consume — descendants that also care about hover (an
                // inner Button inside a hoverable row) still get to see it.
                EventResult::Ignored
            }
            WidgetEvent::MouseLeave => {
                self.hovered = false;
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}
