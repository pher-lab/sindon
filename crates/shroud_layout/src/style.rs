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

    /// Column direction (vertical stacking). Cross axis is horizontal.
    ///
    /// Default cross-axis alignment is `Stretch`, which makes children fill
    /// the column's width. Use [`Self::align_center`] for horizontal centering
    /// (note: this collapses children to min-content width — see
    /// [`Self::align_center`] for the workaround).
    pub fn column(mut self) -> Self {
        self.style.flex_direction = FlexDirection::Column;
        self
    }

    /// Row direction (horizontal, default). Cross axis is vertical.
    ///
    /// Default cross-axis alignment is `Stretch`, which makes every child
    /// expand to the height of the tallest sibling — fine for equal-height
    /// cards, but visually awkward when mixing different-height widgets in a
    /// header (e.g. a 28pt title next to a button: the button's box stretches
    /// to title height and its label appears off-center). Apply
    /// [`Self::align_center`] on the row to size each child to its own height
    /// and vertically center them instead.
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

    /// Maximum width in pixels. Acts as a clamp: the node grows up to this
    /// width and no further, even when its parent offers more space. Useful
    /// for typography (`max_width(640.0)` for readable line length) and for
    /// constraining a centered card on a wide window (Tailwind `max-w-md` ≈
    /// 448 px).
    pub fn max_width(mut self, px: f32) -> Self {
        self.style.max_size.width = length(px);
        self
    }

    /// Maximum height in pixels.
    pub fn max_height(mut self, px: f32) -> Self {
        self.style.max_size.height = length(px);
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

    /// Center items on the main axis only (column → vertical, row →
    /// horizontal). Unlike [`Self::center`], this leaves cross-axis alignment
    /// at the default `Stretch`, so children retain their natural width in a
    /// column. Use this when you want vertical centering without collapsing
    /// children to min-content.
    pub fn justify_center(self) -> Self {
        self.justify_content(JustifyContent::Center)
    }

    /// Center items on both axes. Note that `align_items: Center` shrinks
    /// each child to its min-content size on the cross axis — for a column
    /// container, this collapses child width and can cause text to wrap
    /// per-glyph. Use [`Self::justify_center`] (with explicit child sizing or
    /// [`Self::max_width`]) when you only want main-axis centering.
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

    /// Set the initial main-axis size (`flex-basis`) in pixels.
    ///
    /// Pair with [`Self::grow`] and [`Self::shrink`] to express CSS-idiomatic
    /// patterns like `flex: 1 1 0` (= `.flex_basis(0.0).grow(1.0)`): the item
    /// starts at zero main size and takes whatever leftover space the row /
    /// column hands out, *without* expanding to its content's natural width
    /// first. This is the right shape for "fluid body column next to a
    /// fixed-width decoration" (e.g. a blockquote bar) — using `width_full()`
    /// instead would have the body claim the row's entire main axis and
    /// squeeze siblings to zero.
    pub fn flex_basis(mut self, px: f32) -> Self {
        self.style.flex_basis = length(px);
        self
    }

    /// Allow flex items that overflow the container's main axis to wrap onto
    /// additional lines (`flex-wrap: wrap` when `true`, `nowrap` when `false`).
    ///
    /// Default is `nowrap` — items stay on a single line and are shrunk per
    /// `flex-shrink`. Use this for tag chips, toolbar-style overflow lists,
    /// or any row of `Container::row()` children where the row should grow
    /// vertically rather than push content off-screen horizontally.
    pub fn flex_wrap(mut self, wrap: bool) -> Self {
        self.style.flex_wrap = if wrap {
            FlexWrap::Wrap
        } else {
            FlexWrap::NoWrap
        };
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
