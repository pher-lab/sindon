//! Convenience builder for Taffy `Style`.

use taffy::prelude::*;
use taffy::style::Overflow;

/// Viewport-relative size intents that Taffy cannot express natively.
///
/// Taffy dimensions are pixels, percentages (of the parent), or `auto` — it
/// has no `vh`/`vw` unit. We stash the requested percentages here and bake
/// them into concrete pixel lengths in [`FlexStyle::resolve_viewport`], which
/// the widget tree calls once the viewport size is known (and again on
/// resize). Each field is a percentage of the *same-axis* viewport extent
/// (CSS `vw` for width fields, `vh` for height fields).
#[derive(Debug, Clone, Default)]
struct ViewportDims {
    width_vw: Option<f32>,
    height_vh: Option<f32>,
    min_width_vw: Option<f32>,
    min_height_vh: Option<f32>,
    max_width_vw: Option<f32>,
    max_height_vh: Option<f32>,
}

impl ViewportDims {
    fn any(&self) -> bool {
        self.width_vw.is_some()
            || self.height_vh.is_some()
            || self.min_width_vw.is_some()
            || self.min_height_vh.is_some()
            || self.max_width_vw.is_some()
            || self.max_height_vh.is_some()
    }
}

/// Builder for common flexbox layout styles.
///
/// Converts to `taffy::Style` via `.build()` or `Into<Style>`.
#[derive(Debug, Clone)]
pub struct FlexStyle {
    style: Style,
    /// Deferred viewport-relative dimensions (`vh`/`vw`). Empty for the vast
    /// majority of nodes; resolved to pixels by [`Self::resolve_viewport`].
    viewport: ViewportDims,
}

impl FlexStyle {
    /// Start with default style (row direction, no size constraints).
    pub fn new() -> Self {
        Self {
            style: Style {
                display: Display::Flex,
                ..Default::default()
            },
            viewport: ViewportDims::default(),
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

    /// Width as a percentage of the **viewport** width (CSS `vw`).
    ///
    /// Unlike [`Self::width_percent`] (relative to the parent), this resolves
    /// against the window's width regardless of ancestor sizing — the idiom
    /// behind Tailwind `w-screen` (`width_vw(100.0)`) and arbitrary
    /// `w-[90vw]`. Baked to pixels by the widget tree once the viewport is
    /// known (and re-baked on resize); a bare [`Self::build`] with no viewport
    /// leaves it unset.
    pub fn width_vw(mut self, pct: f32) -> Self {
        self.viewport.width_vw = Some(pct);
        self
    }

    /// Height as a percentage of the **viewport** height (CSS `vh`).
    ///
    /// The viewport-relative counterpart to [`Self::height_percent`] — the
    /// idiom behind Tailwind `h-screen` (`height_vh(100.0)`). See
    /// [`Self::width_vw`] for how/when it resolves.
    pub fn height_vh(mut self, pct: f32) -> Self {
        self.viewport.height_vh = Some(pct);
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

    /// Minimum width as a percentage of the **viewport** width (CSS
    /// `min-w-[..vw]`). See [`Self::width_vw`] for resolution semantics.
    pub fn min_width_vw(mut self, pct: f32) -> Self {
        self.viewport.min_width_vw = Some(pct);
        self
    }

    /// Minimum height as a percentage of the **viewport** height — the idiom
    /// behind Tailwind `min-h-screen` (`min_height_vh(100.0)`). See
    /// [`Self::width_vw`] for resolution semantics.
    pub fn min_height_vh(mut self, pct: f32) -> Self {
        self.viewport.min_height_vh = Some(pct);
        self
    }

    /// Maximum width in pixels. Acts as a clamp: the node grows up to this
    /// width and no further, even when its parent offers more space. Useful
    /// for typography (`max_width(640.0)` for readable line length).
    ///
    /// For a centered card, prefer a definite [`Self::width`] plus
    /// [`Self::margin_x_auto`] over `width_full().max_width(...)`: the latter
    /// resolves to a *percentage* width, and Taffy then measures wrappable
    /// content (text) at the un-clamped percentage width — so a long line
    /// reports a one-line height, the box under-allocates, and the wrapped
    /// tail overflows onto the next widget.
    pub fn max_width(mut self, px: f32) -> Self {
        self.style.max_size.width = length(px);
        self
    }

    /// Maximum height in pixels.
    pub fn max_height(mut self, px: f32) -> Self {
        self.style.max_size.height = length(px);
        self
    }

    /// Maximum width as a percentage of the **viewport** width (CSS
    /// `max-w-[..vw]`). See [`Self::width_vw`] for resolution semantics.
    pub fn max_width_vw(mut self, pct: f32) -> Self {
        self.viewport.max_width_vw = Some(pct);
        self
    }

    /// Maximum height as a percentage of the **viewport** height — the idiom
    /// behind Tailwind `max-h-[80vh]`, i.e. a modal card that never grows past
    /// 80% of the window height and scrolls its body instead. Resolves against
    /// the viewport, not the parent, so it holds even for a centered layer
    /// whose parent is shrink-to-fit. See [`Self::width_vw`].
    pub fn max_height_vh(mut self, pct: f32) -> Self {
        self.viewport.max_height_vh = Some(pct);
        self
    }

    /// Preferred aspect ratio (`width / height`). When one axis is resolved
    /// (by a definite size, a percentage, or flex stretching), Taffy derives
    /// the other from this ratio instead of from content measurement.
    ///
    /// Pair with [`Self::max_width`] for a "responsive image" that fills the
    /// available width up to a cap and scales its height to match, rather than
    /// pinning a fixed box that overflows a narrower container.
    pub fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.style.aspect_ratio = Some(ratio);
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

    /// Auto left/right margins — the flexbox idiom for horizontally centering
    /// a block with a definite or capped width (CSS `margin-inline: auto`).
    pub fn margin_x_auto(mut self) -> Self {
        self.style.margin.left = auto();
        self.style.margin.right = auto();
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

    /// Distribute children along the main axis (CSS `justify-content`).
    ///
    /// The general form of [`Self::justify_center`] — see [`Justify`] for the
    /// full range (`Start` / `End` / `SpaceBetween` / …). Maps the shroud
    /// enum to Taffy so the widget-facing API stays free of Taffy types.
    pub fn justify(self, justify: Justify) -> Self {
        self.justify_content(justify.into())
    }

    /// Align children on the cross axis (CSS `align-items`).
    ///
    /// The general form of [`Self::align_center`] — see [`Align`] for the full
    /// range (`Start` / `End` / `Stretch`). Note the same min-content caveat as
    /// [`Self::align_center`]: `Center` / `Start` / `End` size each child to its
    /// own cross extent, while the default `Stretch` fills the cross axis.
    pub fn align(self, align: Align) -> Self {
        self.align_items(align.into())
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
    /// per-glyph. To horizontally center a fixed- or capped-width child, set
    /// the child's own [`Self::margin_x_auto`] instead of centering via the
    /// parent.
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

    /// Mark this node a scroll container (`overflow: hidden` on both axes).
    ///
    /// Two effects matter for a viewport that fills flex space:
    /// - Its **automatic minimum size becomes 0**, so a flex parent can size
    ///   it smaller than its content (the default `overflow: visible` forces
    ///   a flex item to be at least as tall as its content, which makes a
    ///   `grow(1.0)` viewport balloon to its overflowing content instead of
    ///   clamping to the allocated space — leaving nothing to scroll).
    /// - Its overflowing content **does not contribute to the parent's scroll
    ///   region**, so intermediate `grow` containers between this node and the
    ///   sized ancestor don't balloon either.
    ///
    /// Visual clipping is handled separately by the widget's paint (this only
    /// affects layout). `Hidden` reserves no scrollbar gutter — unlike
    /// `Overflow::Scroll` — so widgets that draw their own scrollbar keep full
    /// control of the gutter.
    pub fn overflow_hidden(mut self) -> Self {
        self.style.overflow = taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
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

    /// Whether any viewport-relative dimension (`vh`/`vw`) is set and still
    /// needs [`Self::resolve_viewport`] to become a concrete pixel length.
    ///
    /// The widget tree checks this to decide which nodes must be re-resolved
    /// on a window resize — nodes without viewport dims keep their
    /// install-time style untouched.
    pub fn has_viewport_dims(&self) -> bool {
        self.viewport.any()
    }

    /// Bake any viewport-relative dimensions (`vh`/`vw`) into concrete pixel
    /// lengths against a viewport of `vw` × `vh` pixels.
    ///
    /// Idempotent and cheap: no-op when [`Self::has_viewport_dims`] is false.
    /// Each set field overwrites the corresponding Taffy `size`/`min_size`/
    /// `max_size` axis, so a later `*_vw`/`*_vh` builder wins over an earlier
    /// pixel/percent one on the same axis. Called by the widget tree once the
    /// viewport is known and again whenever it changes.
    pub fn resolve_viewport(mut self, vw: f32, vh: f32) -> Self {
        let vp = &self.viewport;
        if let Some(p) = vp.width_vw {
            self.style.size.width = length(vw * p / 100.0);
        }
        if let Some(p) = vp.height_vh {
            self.style.size.height = length(vh * p / 100.0);
        }
        if let Some(p) = vp.min_width_vw {
            self.style.min_size.width = length(vw * p / 100.0);
        }
        if let Some(p) = vp.min_height_vh {
            self.style.min_size.height = length(vh * p / 100.0);
        }
        if let Some(p) = vp.max_width_vw {
            self.style.max_size.width = length(vw * p / 100.0);
        }
        if let Some(p) = vp.max_height_vh {
            self.style.max_size.height = length(vh * p / 100.0);
        }
        self
    }

    /// Build the Taffy Style.
    ///
    /// Note: any unresolved viewport-relative dimensions (`vh`/`vw`) are
    /// dropped here — call [`Self::resolve_viewport`] first if the style
    /// carries them. In the running app the widget tree does this for you.
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

/// Main-axis distribution of a flex container's children (CSS
/// `justify-content`), mirroring the Tailwind `justify-*` utilities.
///
/// The shroud-native counterpart to Taffy's `JustifyContent`, so widgets can
/// expose the full range (`Container::justify`) without leaking Taffy types
/// into their API. Pair with [`Align`] for the cross axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justify {
    /// Pack children at the main-axis start (`justify-start`).
    Start,
    /// Center children on the main axis (`justify-center`).
    Center,
    /// Pack children at the main-axis end (`justify-end`).
    End,
    /// Even space *between* children, none at the ends (`justify-between`) —
    /// the idiom for "title left, actions right" header rows.
    SpaceBetween,
    /// Equal space around each child, so the end gaps are half the size of
    /// the gaps between children (`justify-around`).
    SpaceAround,
    /// Equal space between children *and* at the ends (`justify-evenly`).
    SpaceEvenly,
}

/// Cross-axis alignment of a flex container's children (CSS `align-items`),
/// mirroring the Tailwind `items-*` utilities.
///
/// The shroud-native counterpart to Taffy's `AlignItems`; the general form of
/// [`FlexStyle::align_center`]. The default (when never set) is `Stretch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Pack children at the cross-axis start (`items-start`).
    Start,
    /// Center children on the cross axis (`items-center`).
    Center,
    /// Pack children at the cross-axis end (`items-end`).
    End,
    /// Stretch children to fill the cross axis (`items-stretch`, the flex
    /// default) — restores stretching after a builder set something else.
    Stretch,
}

impl From<Justify> for JustifyContent {
    fn from(j: Justify) -> Self {
        match j {
            Justify::Start => JustifyContent::Start,
            Justify::Center => JustifyContent::Center,
            Justify::End => JustifyContent::End,
            Justify::SpaceBetween => JustifyContent::SpaceBetween,
            Justify::SpaceAround => JustifyContent::SpaceAround,
            Justify::SpaceEvenly => JustifyContent::SpaceEvenly,
        }
    }
}

impl From<Align> for AlignItems {
    fn from(a: Align) -> Self {
        match a {
            Align::Start => AlignItems::Start,
            Align::Center => AlignItems::Center,
            Align::End => AlignItems::End,
            Align::Stretch => AlignItems::Stretch,
        }
    }
}
