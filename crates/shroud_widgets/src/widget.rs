//! Widget trait — the core abstraction for UI elements.

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::paint::PaintContext;
use shroud_core::{Rect, SecurityLevel, Size, Theme};
use shroud_layout::FlexStyle;
use shroud_text::TextEngine;

/// The core trait that all widgets implement.
///
/// Widgets produce layout styles, paint themselves, and handle events.
/// They do not own their children — the `WidgetTree` manages parent-child
/// relationships separately.
/// Context passed to `Widget::measure` during layout.
///
/// Provides access to the shared `TextEngine` (for shaping text-bearing
/// widgets) and the active `Theme` (for default font sizes etc.) so widgets
/// can report their intrinsic size before painting.
pub struct MeasureContext<'a> {
    pub text_engine: &'a mut TextEngine,
    pub theme: &'a Theme,
}

impl<'a> MeasureContext<'a> {
    pub fn new(text_engine: &'a mut TextEngine, theme: &'a Theme) -> Self {
        Self { text_engine, theme }
    }
}

pub trait Widget {
    /// The security level of this widget. Propagates to children.
    fn security_level(&self) -> SecurityLevel {
        SecurityLevel::Normal
    }

    /// Return the flex style for layout computation.
    fn style(&self) -> FlexStyle;

    /// Report this widget's intrinsic size for flex layout.
    ///
    /// Returning `None` (the default) means the widget has no intrinsic size
    /// and should be sized entirely by its flex style / parent constraints.
    ///
    /// Returning `Some(size)` tells the layout engine how large this widget
    /// wants to be, which is essential for leaf widgets like `TextWidget`
    /// and `Button` — without it, `Container::column().center()` would
    /// collapse children to width 0 (since `align_items: Center` does not
    /// stretch on the cross axis).
    ///
    /// `available_width` is the content-area width budgeted for this widget
    /// (already excludes padding/border from Taffy's perspective). Widgets
    /// that wrap text should use it as a shaping `max_width`.
    ///
    /// Returned size is the *content* size — Taffy adds padding and border
    /// on top automatically.
    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        None
    }

    /// Paint this widget into the paint context.
    ///
    /// `layout` is the computed absolute rectangle for this widget.
    fn paint(&self, layout: Rect, ctx: &mut PaintContext);

    /// Called by the tree before painting this widget's children.
    ///
    /// Widgets that establish a new clip region or coordinate transform
    /// (e.g. `ScrollView`) should push their state here so that descendants
    /// inherit it while being painted.
    fn paint_pre_children(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    /// Called by the tree after painting this widget's children.
    ///
    /// Should pop any state pushed in `paint_pre_children`.
    fn paint_post_children(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    /// The coordinate transform that hit-testing should add to the cursor
    /// position before descending into this widget's children.
    ///
    /// Returns `(dx, dy)` where positive values compensate for a paint-side
    /// offset. `ScrollView` returns `(0, scroll_y)` so a cursor at screen y
    /// maps to child-space y + scroll_y. Default is `(0, 0)`.
    fn scroll_offset(&self) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Handle an event, returning whether it was consumed.
    fn event(
        &mut self,
        _event: &WidgetEvent,
        _layout: Rect,
        _ctx: &mut EventContext,
    ) -> EventResult {
        EventResult::Ignored
    }
}
