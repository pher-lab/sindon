//! Convenience builder for Taffy `Style`.

use taffy::prelude::*;

/// Builder for common flexbox layout styles.
///
/// Converts to `taffy::Style` via `.build()` or `Into<Style>`.
#[derive(Debug, Clone)]
pub struct FlexStyle {
    style: Style,
}

impl FlexStyle {
    /// Start with default style (row direction, no size constraints).
    pub fn new() -> Self {
        Self {
            style: Style {
                display: Display::Flex,
                ..Default::default()
            },
        }
    }

    /// Column direction (vertical stacking).
    pub fn column(mut self) -> Self {
        self.style.flex_direction = FlexDirection::Column;
        self
    }

    /// Row direction (horizontal, default).
    pub fn row(mut self) -> Self {
        self.style.flex_direction = FlexDirection::Row;
        self
    }

    /// Fixed width in pixels.
    pub fn width(mut self, px: f32) -> Self {
        self.style.size.width = length(px);
        self
    }

    /// Fixed height in pixels.
    pub fn height(mut self, px: f32) -> Self {
        self.style.size.height = length(px);
        self
    }

    /// Width as percentage of parent.
    pub fn width_percent(mut self, pct: f32) -> Self {
        self.style.size.width = percent(pct / 100.0);
        self
    }

    /// Height as percentage of parent.
    pub fn height_percent(mut self, pct: f32) -> Self {
        self.style.size.height = percent(pct / 100.0);
        self
    }

    /// Width fills available space (flex-grow).
    pub fn width_full(mut self) -> Self {
        self.style.size.width = percent(1.0);
        self
    }

    /// Height fills available space.
    pub fn height_full(mut self) -> Self {
        self.style.size.height = percent(1.0);
        self
    }

    /// Minimum width in pixels.
    pub fn min_width(mut self, px: f32) -> Self {
        self.style.min_size.width = length(px);
        self
    }

    /// Minimum height in pixels.
    pub fn min_height(mut self, px: f32) -> Self {
        self.style.min_size.height = length(px);
        self
    }

    /// Uniform padding on all sides.
    pub fn padding(mut self, px: f32) -> Self {
        let val = length(px);
        self.style.padding = Rect {
            left: val,
            right: val,
            top: val,
            bottom: val,
        };
        self
    }

    /// Padding per side: top, right, bottom, left.
    pub fn padding_trbl(mut self, top: f32, right: f32, bottom: f32, left: f32) -> Self {
        self.style.padding = Rect {
            left: length(left),
            right: length(right),
            top: length(top),
            bottom: length(bottom),
        };
        self
    }

    /// Uniform margin on all sides.
    pub fn margin(mut self, px: f32) -> Self {
        let val = length(px);
        self.style.margin = Rect {
            left: val,
            right: val,
            top: val,
            bottom: val,
        };
        self
    }

    /// Gap between children (both row and column gap).
    pub fn gap(mut self, px: f32) -> Self {
        self.style.gap = Size {
            width: length(px),
            height: length(px),
        };
        self
    }

    /// Align items on the cross axis.
    pub fn align_items(mut self, align: AlignItems) -> Self {
        self.style.align_items = Some(align);
        self
    }

    /// Justify content on the main axis.
    pub fn justify_content(mut self, justify: JustifyContent) -> Self {
        self.style.justify_content = Some(justify);
        self
    }

    /// Center items on the cross axis only.
    pub fn align_center(self) -> Self {
        self.align_items(AlignItems::Center)
    }

    /// Center items on both axes.
    pub fn center(self) -> Self {
        self.align_items(AlignItems::Center)
            .justify_content(JustifyContent::Center)
    }

    /// Flex grow factor.
    pub fn grow(mut self, factor: f32) -> Self {
        self.style.flex_grow = factor;
        self
    }

    /// Flex shrink factor.
    pub fn shrink(mut self, factor: f32) -> Self {
        self.style.flex_shrink = factor;
        self
    }

    /// Set `display: none`. The node is removed from the layout flow —
    /// it takes zero space and Taffy cascades this to descendants.
    ///
    /// Used by `WidgetTree` to honor `Widget::visible() == false`. Widget
    /// authors normally reach this via `Container::visible(...)` /
    /// `Button::visible(...)` rather than calling this directly.
    pub fn display_none(mut self) -> Self {
        self.style.display = Display::None;
        self
    }

    /// Build the Taffy Style.
    pub fn build(self) -> Style {
        self.style
    }
}

impl Default for FlexStyle {
    fn default() -> Self {
        Self::new()
    }
}

impl From<FlexStyle> for Style {
    fn from(fs: FlexStyle) -> Self {
        fs.build()
    }
}
