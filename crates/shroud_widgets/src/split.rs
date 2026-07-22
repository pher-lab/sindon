//! Split pane — two resizable regions divided by a draggable handle.
//!
//! A [`SplitPane`] lays out two child regions along one axis (side by side for
//! [`horizontal`](SplitPane::horizontal), stacked for
//! [`vertical`](SplitPane::vertical)) with a divider between them the user can
//! drag to reapportion the space. The split position is a *ratio* in `[0, 1]`
//! (the fraction the first pane gets), held in a [`Signal<f32>`] so an app can
//! persist or observe it.
//!
//! # How it resizes
//!
//! The two panes are `flex: <grow> 0` items whose grow factors are the ratio and
//! its complement, so they divide the space after the divider exactly by ratio.
//! Dragging the divider rewrites the ratio and the panes reflow — which works
//! because each pane opts into [`Widget::style_is_dynamic`], so the layout
//! re-reads its `flex-grow` every frame instead of only when visibility changes.
//! The enclosing container records its own rect each paint, giving the divider
//! the geometry it needs to turn a cursor position into a ratio.
//!
//! # Building
//!
//! [`SplitPane::build`] mounts the machinery into the tree and hands back the
//! two pane node ids to populate:
//!
//! ```ignore
//! let (left, right) = SplitPane::horizontal().ratio(0.35).build(&mut tree, root);
//! tree.add_child(left, sidebar);
//! tree.add_child(right, editor);
//! ```
//!
//! Each pane clips its content to its own box, so a pane narrower than its
//! content hides the overflow rather than bleeding across the divider. Put a
//! [`ScrollView`](crate::ScrollView) inside a pane whose content can exceed it.

use std::cell::Cell;
use std::rc::Rc;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::interaction::InteractionState;
use crate::paint::PaintContext;
use crate::tree::WidgetTree;
use crate::widget::Widget;
use shroud_core::{Color, Point, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::Signal;

/// Layout-space the divider occupies (a wide, easy-to-grab hit target; the
/// visible line is thinner and centered).
const DIVIDER_THICKNESS: f32 = 8.0;
const DEFAULT_RATIO: f32 = 0.5;
const DEFAULT_MIN_RATIO: f32 = 0.1;

/// The axis a [`SplitPane`] divides along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Panes side by side, divided by a vertical handle dragged left/right.
    Horizontal,
    /// Panes stacked, divided by a horizontal handle dragged up/down.
    Vertical,
}

#[derive(Debug, Clone, Copy)]
enum Side {
    First,
    Second,
}

/// State shared by the three widgets that make up one split: the container
/// (writes its rect), the two panes (read the ratio for their grow factor), and
/// the divider (reads the rect + writes the ratio on drag).
struct SplitState {
    orientation: Orientation,
    /// First pane's fraction of the usable space, in `[min_ratio, 1 - min_ratio]`.
    ratio: Signal<f32>,
    min_ratio: f32,
    /// The container's most recent absolute rect, refreshed every paint — the
    /// coordinate frame the divider maps a cursor position into.
    rect: Cell<Rect>,
}

impl SplitState {
    fn clamp(&self, r: f32) -> f32 {
        r.clamp(self.min_ratio, 1.0 - self.min_ratio)
    }

    /// The clamped first-pane fraction.
    fn ratio(&self) -> f32 {
        self.clamp(self.ratio.get())
    }
}

/// A two-region resizable split. Configure with the builder methods, then
/// [`build`](Self::build) it into the tree.
pub struct SplitPane {
    orientation: Orientation,
    ratio: Signal<f32>,
    min_ratio: f32,
    divider_color: Option<Color>,
    divider_active_color: Option<Color>,
}

impl SplitPane {
    fn new(orientation: Orientation) -> Self {
        Self {
            orientation,
            ratio: Signal::new(DEFAULT_RATIO),
            min_ratio: DEFAULT_MIN_RATIO,
            divider_color: None,
            divider_active_color: None,
        }
    }

    /// A left/right split with a vertical divider.
    pub fn horizontal() -> Self {
        Self::new(Orientation::Horizontal)
    }

    /// A top/bottom split with a horizontal divider.
    pub fn vertical() -> Self {
        Self::new(Orientation::Vertical)
    }

    /// Set the initial split ratio — the first pane's fraction of the space, in
    /// `[0, 1]` (clamped). Ignored once [`bind`](Self::bind) is attached (the
    /// signal owns it).
    pub fn ratio(self, ratio: f32) -> Self {
        self.ratio.set(ratio.clamp(0.0, 1.0));
        self
    }

    /// Bind the split position two-way to a [`Signal<f32>`]: external writes move
    /// the divider, and every drag writes back — so an app can persist the ratio
    /// and restore it.
    pub fn bind(mut self, ratio: Signal<f32>) -> Self {
        self.ratio = ratio;
        self
    }

    /// The smallest fraction either pane may shrink to (drags clamp to
    /// `[min, 1 - min]`). Clamped into `[0, 0.49]`.
    pub fn min_ratio(mut self, min: f32) -> Self {
        self.min_ratio = min.clamp(0.0, 0.49);
        self
    }

    /// Override the resting divider line color. `None` reads
    /// `theme.colors.divider`.
    pub fn divider_color(mut self, color: Color) -> Self {
        self.divider_color = Some(color);
        self
    }

    /// Override the divider line color while hovered / dragged. `None` reads
    /// `theme.colors.primary`.
    pub fn divider_active_color(mut self, color: Color) -> Self {
        self.divider_active_color = Some(color);
        self
    }

    /// The signal backing the split ratio, so an app can read or persist it.
    pub fn ratio_signal(&self) -> Signal<f32> {
        self.ratio
    }

    /// Mount the split under `parent` and return the two pane node ids
    /// (`(first, second)`) to add content into.
    pub fn build(self, tree: &mut WidgetTree, parent: usize) -> (usize, usize) {
        let state = Rc::new(SplitState {
            orientation: self.orientation,
            ratio: self.ratio,
            min_ratio: self.min_ratio,
            rect: Cell::new(Rect::ZERO),
        });

        let container = tree.add_child(
            parent,
            SplitContainer {
                state: Rc::clone(&state),
            },
        );
        let first = tree.add_child(
            container,
            SplitPaneSide {
                state: Rc::clone(&state),
                side: Side::First,
            },
        );
        tree.add_child(
            container,
            SplitDivider {
                state: Rc::clone(&state),
                interaction: InteractionState::default(),
                dragging: false,
                line: self.divider_color,
                active: self.divider_active_color,
            },
        );
        let second = tree.add_child(
            container,
            SplitPaneSide {
                state,
                side: Side::Second,
            },
        );
        (first, second)
    }
}

/// The flex container holding `[first, divider, second]`. Its only job beyond
/// being a row/column is to record its own rect for the divider to map against.
struct SplitContainer {
    state: Rc<SplitState>,
}

impl Widget for SplitContainer {
    fn style(&self) -> FlexStyle {
        let base = FlexStyle::new().width_full().height_full();
        match self.state.orientation {
            Orientation::Horizontal => base.row(),
            Orientation::Vertical => base.column(),
        }
    }

    fn paint(&self, layout: Rect, _ctx: &mut PaintContext) {
        self.state.rect.set(layout);
    }
}

/// One pane. A `flex: <ratio> 0` box (so it takes exactly its fraction of the
/// space, not content + a share of the slack) that clips its content.
struct SplitPaneSide {
    state: Rc<SplitState>,
    side: Side,
}

impl Widget for SplitPaneSide {
    fn style_is_dynamic(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        let ratio = self.state.ratio();
        let grow = match self.side {
            Side::First => ratio,
            Side::Second => 1.0 - ratio,
        };
        // `flex-basis: 0` + a grow factor is `flex: <grow> 0` — the two panes
        // divide the space strictly by their grow ratio. `overflow_hidden` keeps
        // a pane from ballooning a hugging ancestor to its content size.
        FlexStyle::new()
            .flex_basis(0.0)
            .grow(grow.max(f32::EPSILON))
            .overflow_hidden()
    }

    fn paint(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    fn paint_pre_children(&self, layout: Rect, ctx: &mut PaintContext) {
        ctx.push_clip(layout);
    }

    fn paint_post_children(&self, _layout: Rect, ctx: &mut PaintContext) {
        ctx.pop_clip();
    }
}

/// The draggable handle between the two panes.
struct SplitDivider {
    state: Rc<SplitState>,
    interaction: InteractionState,
    dragging: bool,
    line: Option<Color>,
    active: Option<Color>,
}

impl SplitDivider {
    /// Map a cursor position (screen space) to a ratio and commit it, using the
    /// container rect the divider was told about.
    fn drag_to(&self, pos: Point) {
        let rect = self.state.rect.get();
        let (origin, extent, cursor) = match self.state.orientation {
            Orientation::Horizontal => (rect.origin.x, rect.size.width, pos.x),
            Orientation::Vertical => (rect.origin.y, rect.size.height, pos.y),
        };
        // The panes share the space left after the divider, so the ratio is
        // measured against that usable extent.
        let usable = (extent - DIVIDER_THICKNESS).max(1.0);
        let ratio = self.state.clamp((cursor - origin) / usable);
        self.state.ratio.set(ratio);
    }
}

impl Widget for SplitDivider {
    fn style(&self) -> FlexStyle {
        match self.state.orientation {
            Orientation::Horizontal => FlexStyle::new().width(DIVIDER_THICKNESS).height_full(),
            Orientation::Vertical => FlexStyle::new().height(DIVIDER_THICKNESS).width_full(),
        }
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let active = self.dragging || self.interaction.hovered;
        let color = if active {
            self.active.unwrap_or(ctx.theme.colors.primary)
        } else {
            self.line.unwrap_or(ctx.theme.colors.divider)
        };
        let thickness = if active { 2.0 } else { 1.0 };
        let rect = match self.state.orientation {
            Orientation::Horizontal => Rect::new(
                layout.origin.x + (layout.size.width - thickness) / 2.0,
                layout.origin.y,
                thickness,
                layout.size.height,
            ),
            Orientation::Vertical => Rect::new(
                layout.origin.x,
                layout.origin.y + (layout.size.height - thickness) / 2.0,
                layout.size.width,
                thickness,
            ),
        };
        ctx.fill_rect(rect, color);
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.interaction.enter(false);
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.interaction.leave();
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                position,
            } => {
                // Grab the pointer so the drag keeps tracking even when the
                // cursor wanders off the thin handle (the same idiom Slider uses).
                self.dragging = true;
                ctx.capture_pointer();
                self.drag_to(*position);
                EventResult::Consumed
            }
            WidgetEvent::MouseMove { position } if self.dragging => {
                self.drag_to(*position);
                EventResult::Consumed
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.dragging {
                    self.dragging = false;
                    ctx.release_pointer();
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(orientation: Orientation, ratio: f32, min: f32) -> Rc<SplitState> {
        Rc::new(SplitState {
            orientation,
            ratio: Signal::new(ratio),
            min_ratio: min,
            rect: Cell::new(Rect::new(0.0, 0.0, 208.0, 100.0)),
        })
    }

    fn divider(state: Rc<SplitState>) -> SplitDivider {
        SplitDivider {
            state,
            interaction: InteractionState::default(),
            dragging: false,
            line: None,
            active: None,
        }
    }

    #[test]
    fn pane_grow_factors_sum_and_split_by_ratio() {
        let st = state(Orientation::Horizontal, 0.25, 0.1);
        let first = SplitPaneSide {
            state: Rc::clone(&st),
            side: Side::First,
        };
        let second = SplitPaneSide {
            state: Rc::clone(&st),
            side: Side::Second,
        };
        // The two panes are `flex: r 0` and `flex: (1-r) 0`, so their grow
        // factors are the ratio and its complement — a 25/75 split here.
        let fg = first.style().build().flex_grow;
        let sg = second.style().build().flex_grow;
        assert!((fg - 0.25).abs() < 1e-6, "first grows by the ratio");
        assert!((sg - 0.75).abs() < 1e-6, "second grows by the complement");
        assert!(
            (fg + sg - 1.0).abs() < 1e-6,
            "the two shares fill the space"
        );
    }

    #[test]
    fn panes_opt_into_dynamic_style() {
        let st = state(Orientation::Horizontal, 0.5, 0.1);
        let pane = SplitPaneSide {
            state: st,
            side: Side::First,
        };
        assert!(
            pane.style_is_dynamic(),
            "a pane must re-read its style so a drag reflows it"
        );
    }

    #[test]
    fn drag_maps_cursor_to_ratio_horizontally() {
        // usable = 208 - 8 = 200, origin x = 0, so a cursor at x=50 is ratio 0.25.
        let st = state(Orientation::Horizontal, 0.5, 0.1);
        divider(Rc::clone(&st)).drag_to(Point::new(50.0, 40.0));
        assert!((st.ratio() - 0.25).abs() < 1e-6);

        // Midpoint.
        divider(Rc::clone(&st)).drag_to(Point::new(100.0, 40.0));
        assert!((st.ratio() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn drag_clamps_to_min_ratio() {
        let st = state(Orientation::Horizontal, 0.5, 0.2);
        // Dragging past the left edge can't shrink the first pane below min.
        divider(Rc::clone(&st)).drag_to(Point::new(-100.0, 40.0));
        assert!((st.ratio() - 0.2).abs() < 1e-6, "clamped to min_ratio");
        // Past the right edge clamps to 1 - min.
        divider(Rc::clone(&st)).drag_to(Point::new(9999.0, 40.0));
        assert!((st.ratio() - 0.8).abs() < 1e-6, "clamped to 1 - min_ratio");
    }

    #[test]
    fn vertical_drag_uses_the_y_axis() {
        // rect is 208 wide x 100 tall; vertical split measures against height:
        // usable = 100 - 8 = 92, cursor y=46 → ratio ≈ 0.5.
        let st = state(Orientation::Vertical, 0.3, 0.1);
        divider(Rc::clone(&st)).drag_to(Point::new(10.0, 46.0));
        assert!((st.ratio() - 46.0 / 92.0).abs() < 1e-6);
    }

    #[test]
    fn bind_makes_the_ratio_two_way() {
        let sig = Signal::new(0.4f32);
        let sp = SplitPane::horizontal().bind(sig);
        assert_eq!(sp.ratio_signal().get(), 0.4);
        // An external write moves the divider's source of truth.
        sig.set(0.7);
        assert_eq!(sp.ratio_signal().get(), 0.7);
    }

    #[test]
    fn divider_drag_updates_a_bound_signal() {
        let sig = Signal::new(0.5f32);
        let st = Rc::new(SplitState {
            orientation: Orientation::Horizontal,
            ratio: sig,
            min_ratio: 0.1,
            rect: Cell::new(Rect::new(0.0, 0.0, 208.0, 100.0)),
        });
        divider(st).drag_to(Point::new(50.0, 40.0));
        assert!(
            (sig.get() - 0.25).abs() < 1e-6,
            "drag writes the bound signal"
        );
    }
}
