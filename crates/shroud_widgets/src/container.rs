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
}

impl Container {
    /// Create a column container (vertical stacking).
    pub fn column() -> Self {
        Self {
            style: FlexStyle::new().column(),
            background: None,
        }
    }

    /// Create a row container (horizontal stacking).
    pub fn row() -> Self {
        Self {
            style: FlexStyle::new().row(),
            background: None,
        }
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

    /// Center children on both axes.
    pub fn center(mut self) -> Self {
        self.style = self.style.center();
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

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        if let Some(bg) = &self.background {
            ctx.fill_rect(layout, bg.get());
        }
    }
}
