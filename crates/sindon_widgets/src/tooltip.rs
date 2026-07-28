//! Hover tooltips — a delayed, click-through tip anchored to its trigger.
//!
//! Wrap whatever should carry a tip in a [`Tooltip`] and add the real widget
//! as its child:
//!
//! ```
//! # use sindon_widgets::tree::WidgetTree;
//! # use sindon_widgets::{Button, Container, Tooltip};
//! # let mut tree = WidgetTree::new();
//! # let row = tree.set_root(Container::row());
//! let cell = tree.add_child(row, Tooltip::new("Bold"));
//! tree.add_child(cell, Button::new("B").on_click(|_ctx| { /* toggle bold */ }));
//! ```
//!
//! The wrapper hugs its child and reports its own layout rect on hover, which
//! is what the bubble anchors to.
//!
//! ## Why the tip is a click-through layer
//!
//! A tip pushed as an ordinary layer would capture all pointer input, so the
//! trigger would never see the `MouseLeave` that dismisses it — the tip would
//! stick or flicker. [`LayerOptions::tooltip`] is non-interactive: it paints on
//! top but event routing skips it, so the trigger keeps receiving hover events.
//!
//! ## Timing
//!
//! The tip appears once the cursor has rested on the trigger for
//! [`DEFAULT_DELAY`] (per-tooltip via [`Tooltip::delay`]), so brushing past an
//! icon does not flash one. Arming votes for a frame at the deadline through
//! the animation pump, so the delay does not depend on the app running a
//! periodic tick — [`WidgetTree::sync_tooltips`](crate::WidgetTree::sync_tooltips)
//! is what actually shows it, and the event loop calls that every frame.
//!
//! ## State
//!
//! At most one tip is up at a time, so the controller is a thread-local
//! singleton (the UI runs single-threaded on the event-loop thread) rather than
//! per-widget state. It records the pushed layer by root index and dismisses
//! *that* index, never "the topmost layer" — a tip is not always topmost (a
//! keyboard shortcut can open a modal above one), and node indices are never
//! recycled, so dismissing a layer that is already gone is a provable no-op
//! rather than a chance to pop somebody else's layer.
//!
//! Teardown paths that never produce a `MouseLeave` are handled at the source:
//! [`WidgetTree::remove`](crate::WidgetTree::remove) cancels the tip when the
//! subtree it drops contains the hovered node, which covers both a rebuilt list
//! and a whole-screen swap. Apps have nothing to reset.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use crate::event::{EventContext, EventResult, WidgetEvent};
use crate::layer::{HAlign, LayerAnchor, LayerOptions, Placement};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::{Reactive, animation::request_frame_at};

/// How long the cursor must rest on a trigger before its tip appears.
pub const DEFAULT_DELAY: Duration = Duration::from_millis(400);

/// Tip text size — a touch smaller than body text.
const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Inset between the bubble's edge and its text.
const BUBBLE_PADDING: f32 = 6.0;
/// Bubble corner radius.
const BUBBLE_RADIUS: f32 = 6.0;

/// A trigger the cursor is resting on, waiting out its delay.
#[derive(Clone)]
pub(crate) struct Pending {
    /// The trigger's layout rect (viewport coords), for anchoring.
    rect: Rect,
    /// Already-resolved tip text.
    text: String,
    /// When the tip becomes due.
    due: Instant,
    placement: Placement,
    font_size: f32,
}

#[derive(Default)]
struct State {
    /// The armed trigger, if any. Cleared on leave.
    pending: Option<Pending>,
    /// Root node index of the tip layer while one is up.
    shown: Option<usize>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// A hover tooltip wrapper.
///
/// Lays out as a row that hugs its child (add exactly one, normally) and shows
/// `text` in a bubble anchored beneath it after the delay. See the module docs.
pub struct Tooltip {
    text: Reactive<String>,
    delay: Duration,
    placement: Placement,
    font_size: f32,
}

impl Tooltip {
    /// Wrap a child with the given tip text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Reactive::Static(text.into()),
            delay: DEFAULT_DELAY,
            placement: Placement::Auto,
            font_size: DEFAULT_FONT_SIZE,
        }
    }

    /// Wrap a child with tip text produced by a closure, read when the tip is
    /// armed. The usual reason is a language switch: the tip picks up the new
    /// translation without the screen being rebuilt.
    pub fn reactive(f: impl Fn() -> String + 'static) -> Self {
        Self {
            text: Reactive::derive(f),
            ..Self::new(String::new())
        }
    }

    /// How long the cursor must rest before the tip appears. Defaults to
    /// [`DEFAULT_DELAY`].
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Where the bubble sits relative to the trigger. Defaults to
    /// [`Placement::Auto`] — below, flipping above near the viewport bottom.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Tip text size. Defaults to 13px.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = px;
        self
    }
}

impl Widget for Tooltip {
    fn style(&self) -> FlexStyle {
        // Pure wrapper: a hugging row, so inserting one around a button does
        // not change how that button lays out.
        FlexStyle::new().row()
    }

    fn paint(&self, _layout: Rect, _ctx: &mut PaintContext) {
        // Nothing of its own: the wrapper is invisible, and the tip itself
        // paints from its own layer. Deliberately *not* a hover highlight —
        // carrying a tip must not restyle the widget underneath.
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                let text = self.text.get();
                if !text.is_empty() {
                    arm(
                        Pending {
                            rect: layout,
                            text,
                            due: Instant::now() + self.delay,
                            placement: self.placement,
                            font_size: self.font_size,
                        },
                        ctx,
                    );
                }
                // Not consumed: a hoverable ancestor still gets to light up.
                EventResult::Ignored
            }
            WidgetEvent::MouseLeave => {
                disarm(ctx);
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Arm a trigger. Any tip still up belongs to a different trigger (a stray
/// enter with no matching leave), so dismiss it first rather than stranding a
/// bubble for something the cursor has left.
fn arm(pending: Pending, ctx: &mut EventContext) {
    let stale = STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.pending = Some(pending);
        st.shown.take()
    });
    if let Some(root) = stale {
        ctx.pop_layer(root);
    }
}

/// The cursor left: drop the pending clock and dismiss any shown tip.
fn disarm(ctx: &mut EventContext) {
    let shown = STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.pending = None;
        st.shown.take()
    });
    if let Some(root) = shown {
        ctx.pop_layer(root);
    }
}

/// Cancel everything and hand back the shown layer's root, if any, for the
/// caller to pop. Used by the tree when a teardown swallows the hovered node,
/// which produces no `MouseLeave` of its own.
pub(crate) fn cancel() -> Option<usize> {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.pending = None;
        st.shown.take()
    })
}

/// The pending tip if its delay has elapsed and nothing is shown yet.
///
/// When one is armed but not yet due this votes for a frame at the deadline,
/// which is what makes the delay independent of any app-side tick. The vote is
/// cast from here — inside the frame, where the animation pump collects it —
/// rather than from the `MouseEnter` handler, whose vote the next frame's
/// reset would discard.
pub(crate) fn due_now() -> Option<Pending> {
    STATE.with(|s| {
        let st = s.borrow();
        if st.shown.is_some() {
            return None;
        }
        let pending = st.pending.as_ref()?;
        if Instant::now() >= pending.due {
            Some(pending.clone())
        } else {
            request_frame_at(pending.due);
            None
        }
    })
}

/// Record the layer that is now showing the pending tip.
pub(crate) fn mark_shown(root: usize) {
    STATE.with(|s| s.borrow_mut().shown = Some(root));
}

/// Layer options + bubble widget for a due tip. Split out so the tree's pump
/// stays a two-liner and every bubble detail lives in this module.
pub(crate) fn bubble_for(pending: &Pending) -> (LayerOptions, TooltipBubble) {
    let options = LayerOptions::tooltip().anchor(LayerAnchor::AnchorRect {
        rect: pending.rect,
        prefer: pending.placement,
        align: HAlign::Start,
    });
    let bubble = TooltipBubble {
        text: pending.text.clone(),
        font_size: pending.font_size,
    };
    (options, bubble)
}

/// The tip bubble: a small raised surface that paints its own text.
///
/// Self-painting rather than a styled container with a `TextWidget` child, for
/// the same reason `Dropdown` paints its trigger label: it can then read the
/// tokens it actually wants (`surface` / `on_surface`) from the live theme at
/// paint time, instead of freezing a color when the layer is built.
///
/// One line by construction — tips are labels, not paragraphs.
pub(crate) struct TooltipBubble {
    text: String,
    font_size: f32,
}

impl Widget for TooltipBubble {
    fn style(&self) -> FlexStyle {
        // Measured leaf: the size comes from `measure`, so no `min_size` here
        // (see `Button::style` for why the two must not both be set).
        FlexStyle::new().padding(BUBBLE_PADDING)
    }

    fn measure(&self, _available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let line_height = self.font_size * 1.2;
        let (w, _h) = ctx
            .text_engine
            .measure_text(&self.text, self.font_size, line_height, None);
        // Taffy adds the padding on top to form the border box.
        Some(Size::new(w.ceil(), line_height.ceil()))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        // Copy the tokens out before the first `&mut ctx` call.
        let surface = ctx.theme.colors.surface;
        let border = ctx.theme.colors.input_border;
        let color = ctx.theme.colors.on_surface;
        ctx.fill_rect_rounded(layout, surface, BUBBLE_RADIUS);
        // Hairline border so the bubble separates from a same-colored surface
        // underneath it — matching the dropdown popover.
        ctx.stroke_rect_rounded(layout, border, BUBBLE_RADIUS, 1.0);

        let line_height = self.font_size * 1.2;
        let shaped = ctx
            .text_engine
            .shape_text(&self.text, self.font_size, line_height, None);
        let text_x = layout.origin.x + BUBBLE_PADDING;
        let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
        for glyph in &shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    text_x + glyph.x,
                    text_y + glyph.y,
                    image,
                    color,
                    glyph.cache_key,
                );
            }
        }
    }
}
