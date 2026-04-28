//! Container widget — a flexbox layout container.

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
    visible: Reactive<bool>,
}

impl Container {
    /// Create a column container (vertical stacking).
    pub fn column() -> Self {
        Self {
            style: FlexStyle::new().column(),
            background: None,
            visible: Reactive::Static(true),
        }
    }

    /// Create a row container (horizontal stacking).
    pub fn row() -> Self {
        Self {
            style: FlexStyle::new().row(),
            background: None,
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
        if let Some(bg) = &self.background {
            ctx.fill_rect(layout, bg.get());
        }
    }
}
