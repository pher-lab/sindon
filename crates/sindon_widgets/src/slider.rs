//! Slider widget — pick an `f32` in a range by dragging a thumb along a track.
//!
//! The continuous cousin of [`Switch`](crate::Switch): where a switch is a
//! two-state toggle, a slider is a value in `[min, max]` — volume, opacity, a
//! font-size setting. It carries the same [`Input`](crate::Input)-style binding
//! API (owned initial value, optional two-way [`Signal<f32>`], `on_change`,
//! `.disabled`), so a settings signal can own the value while the widget stays a
//! thin view.
//!
//! Pointer drag uses the tree's pointer capture (the same mechanism
//! [`Input`](crate::Input) uses for drag-select): a press anywhere on the track
//! jumps the thumb there and captures the pointer, so the drag keeps tracking
//! even when the cursor wanders off the widget vertically. Once focused (a click
//! focuses it, or Tab), Left/Down nudge down by a step, Right/Up nudge up, and
//! Home/End jump to the ends.

use std::cell::Cell;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::interaction::{InteractionState, dim_over};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{AccessAction, AccessNode, AccessRange, AccessRole, Color, Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::{Reactive, Signal};

/// Thumb diameter in pixels. Also the widget's intrinsic height, so the thumb
/// is never clipped by the row.
const THUMB_D: f32 = 18.0;
/// Track thickness in pixels — a thin pill the thumb rides along.
const TRACK_THICKNESS: f32 = 6.0;
/// Intrinsic min width so a slider in a hug (non-stretch) context is still
/// usable; a stretch parent (a settings column) overrides it to full width.
const MIN_WIDTH: f32 = 140.0;

/// Handler for [`Slider::on_change`]. Receives the new value and the dispatch
/// context for queuing tree mutations.
type ChangeHandler = Box<dyn FnMut(f32, &mut EventContext)>;

/// A horizontal slider selecting an `f32` in `[min, max]`.
///
/// # Example (conceptual)
/// ```ignore
/// let opacity = Signal::new(0.8);
/// let sl = Slider::new(0.0, 1.0)
///     .bind(opacity)
///     .step(0.05)
///     .on_change(|v, _ctx| println!("opacity: {v:.2}"));
/// ```
pub struct Slider {
    min: f32,
    max: f32,
    /// Snap granularity. `None` = continuous drag; `Some(step)` snaps drag and
    /// keyboard moves to the `min + n*step` grid.
    step: Option<f32>,
    /// Owned mirror of the value. The bound [`source`](Self::source) is the
    /// source of truth on read when present; this keeps an unbound slider
    /// stateful.
    value: Cell<f32>,
    /// Optional bound signal. Read fresh each paint and written on every move.
    source: Option<Signal<f32>>,
    on_change: Option<ChangeHandler>,
    disabled: Reactive<bool>,
    /// Hover / focus flags (see [`InteractionState`]). The press latch is
    /// unused — the drag runs off its own [`dragging`](Self::dragging) flag so
    /// a `MouseLeave` mid-drag can't cancel it (the pointer is captured).
    state: InteractionState,
    /// A drag is in flight: set on press, cleared only on release. Independent
    /// of hover so the captured drag survives the cursor leaving the widget.
    dragging: bool,
    // Colours (None = read from theme each frame).
    track_color: Option<Color>,
    fill_color: Option<Color>,
    thumb_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl Slider {
    /// Create a slider over `[min, max]`, resting at `min`. If `max <= min` the
    /// range is degenerate and the slider is a fixed point at `min`.
    pub fn new(min: f32, max: f32) -> Self {
        Self {
            min,
            max,
            step: None,
            value: Cell::new(min),
            source: None,
            on_change: None,
            disabled: Reactive::Static(false),
            state: InteractionState::default(),
            dragging: false,
            track_color: None,
            fill_color: None,
            thumb_color: None,
            focus_ring_color: None,
        }
    }

    /// Set the initial value (clamped to the range). Ignored once a
    /// [`bind`](Self::bind) signal is attached — the signal then owns it.
    pub fn value(self, v: f32) -> Self {
        self.value.set(v.clamp(self.min, self.max));
        self
    }

    /// Bind the slider two-way to a [`Signal<f32>`]. The signal becomes the
    /// source of truth: external writes are reflected on the next paint, and
    /// every move writes back to it (and still fires
    /// [`on_change`](Self::on_change)).
    pub fn bind(mut self, signal: Signal<f32>) -> Self {
        self.value.set(signal.get().clamp(self.min, self.max));
        self.source = Some(signal);
        self
    }

    /// Snap drag and keyboard moves to a `min + n*step` grid. Non-positive
    /// values are ignored (left continuous).
    pub fn step(mut self, step: f32) -> Self {
        self.step = if step > 0.0 { Some(step) } else { None };
        self
    }

    /// Set a callback fired whenever the value changes.
    pub fn on_change(mut self, f: impl FnMut(f32, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Gate the slider on a disabled state (reactive). While `true` it drops
    /// hover feedback, is skipped by Tab, ignores drag and keys, and paints
    /// muted — the track / fill / thumb faded toward the surface, kept opaque so
    /// the thumb still covers the track rather than turning to glass.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Override the track (groove) colour. `None` reads `theme.colors.outline`.
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = Some(color);
        self
    }

    /// Override the filled-portion colour. `None` reads `theme.colors.primary`.
    pub fn fill_color(mut self, color: Color) -> Self {
        self.fill_color = Some(color);
        self
    }

    /// Override the thumb colour. `None` reads `theme.colors.on_primary`.
    pub fn thumb_color(mut self, color: Color) -> Self {
        self.thumb_color = Some(color);
        self
    }

    /// Override the keyboard-focus ring colour. `None` reads
    /// `theme.focus.ring_color`.
    pub fn focus_ring_color(mut self, color: Color) -> Self {
        self.focus_ring_color = Some(color);
        self
    }

    /// The current value (reads the bound signal if attached), clamped to range.
    pub fn get(&self) -> f32 {
        let raw = match &self.source {
            Some(s) => s.get(),
            None => self.value.get(),
        };
        raw.clamp(self.min, self.max)
    }

    /// Whether this slider currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    /// Snap `v` to the step grid (if a step is set) and clamp to range.
    fn snap(&self, v: f32) -> f32 {
        let v = match self.step {
            Some(step) if step > 0.0 => {
                let n = ((v - self.min) / step).round();
                self.min + n * step
            }
            _ => v,
        };
        v.clamp(self.min, self.max)
    }

    /// Map a pointer x (screen space) to a value, given the widget's layout —
    /// the thumb centre travels between `x + r` and `x + width − r` so it never
    /// overflows the track ends.
    fn value_at_x(&self, x: f32, layout: Rect) -> f32 {
        let r = THUMB_D / 2.0;
        let usable = (layout.size.width - THUMB_D).max(1.0);
        let t = ((x - (layout.origin.x + r)) / usable).clamp(0.0, 1.0);
        self.snap(self.min + t * (self.max - self.min))
    }

    /// The step used for a keyboard nudge — the explicit step, or 1/100 of the
    /// range when continuous.
    fn key_step(&self) -> f32 {
        self.step
            .unwrap_or_else(|| ((self.max - self.min) / 100.0).max(f32::EPSILON))
    }

    /// Commit a new value: snap, clamp, and — only if it actually changed —
    /// write the mirror, the bound signal, and fire the change handler. The
    /// single write path shared by drag, click, and keyboard.
    fn commit(&mut self, v: f32, ctx: &mut EventContext) {
        let prev = self.get();
        let v = self.snap(v);
        if (v - prev).abs() < f32::EPSILON {
            return;
        }
        self.value.set(v);
        if let Some(s) = &self.source {
            s.set(v);
        }
        if let Some(handler) = &mut self.on_change {
            handler(v, ctx);
        }
    }

    /// Nudge the value by `dir` (±1) steps.
    fn nudge(&mut self, dir: f32, ctx: &mut EventContext) {
        let v = self.get() + dir * self.key_step();
        self.commit(v, ctx);
    }
}

impl Widget for Slider {
    fn focusable(&self) -> bool {
        !self.disabled.get()
    }

    fn accessibility(&self) -> Option<AccessNode> {
        Some(
            AccessNode::new(AccessRole::Slider)
                .numeric(AccessRange {
                    min: self.min as f64,
                    max: self.max as f64,
                    now: self.get() as f64,
                })
                .disabled(self.disabled.get()),
        )
    }

    fn accessibility_action(
        &mut self,
        action: AccessAction,
        _option: Option<usize>,
        _layout: Rect,
        ctx: &mut EventContext,
    ) -> EventResult {
        if self.disabled.get() {
            return EventResult::Ignored;
        }
        // Increment / Decrement land on the same nudge the arrow keys use, so
        // an AT steps by the widget's own step grid. `SetValue` goes through
        // `commit`, which snaps and clamps it exactly as a drag would — an out
        // of range value from the AT can't push the slider off its rails.
        match action {
            AccessAction::Increment => self.nudge(1.0, ctx),
            AccessAction::Decrement => self.nudge(-1.0, ctx),
            AccessAction::SetValue(v) => self.commit(v as f32, ctx),
            _ => return EventResult::Ignored,
        }
        EventResult::Consumed
    }

    fn style(&self) -> FlexStyle {
        // Measured leaf — no `min_size` here (see `measure` / the Button note).
        FlexStyle::new()
    }

    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        // Intrinsic height = thumb diameter; a stretch parent widens us.
        Some(Size::new(MIN_WIDTH, THUMB_D))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let disabled = self.disabled.get();
        let v = self.get();
        let t = if self.max > self.min {
            (v - self.min) / (self.max - self.min)
        } else {
            0.0
        };

        let r = THUMB_D / 2.0;
        let mid_y = layout.origin.y + layout.size.height / 2.0;
        let track_y = mid_y - TRACK_THICKNESS / 2.0;
        let track_radius = TRACK_THICKNESS / 2.0;

        let usable = (layout.size.width - THUMB_D).max(1.0);
        let thumb_cx = layout.origin.x + r + t * usable;

        let mut track = self.track_color.unwrap_or(ctx.theme.colors.outline);
        let mut fill = self.fill_color.unwrap_or(ctx.theme.colors.primary);
        let mut thumb = self.thumb_color.unwrap_or(ctx.theme.colors.on_primary);
        if disabled {
            let surface = ctx.theme.colors.surface;
            track = dim_over(track, surface);
            fill = dim_over(fill, surface);
            thumb = dim_over(thumb, surface);
        }

        // Groove (full width), then the filled portion up to the thumb centre.
        ctx.fill_rect_rounded(
            Rect::new(layout.origin.x, track_y, layout.size.width, TRACK_THICKNESS),
            track,
            track_radius,
        );
        let fill_w = (thumb_cx - layout.origin.x).max(0.0);
        if fill_w > 0.0 {
            ctx.fill_rect_rounded(
                Rect::new(layout.origin.x, track_y, fill_w, TRACK_THICKNESS),
                fill,
                track_radius,
            );
        }

        // Thumb.
        let thumb_rect = Rect::new(thumb_cx - r, mid_y - r, THUMB_D, THUMB_D);
        ctx.fill_rect_rounded(thumb_rect, thumb, r);

        if self.state.focused && !disabled && ctx.focus_visible() {
            ctx.paint_focus_ring(thumb_rect, self.focus_ring_color, r);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        let disabled = self.disabled.get();
        match event {
            // Clearing transitions run even while disabled — see `InteractionState`.
            WidgetEvent::MouseLeave => {
                self.state.leave();
                EventResult::Consumed
            }
            WidgetEvent::FocusLost => {
                self.state.focus_lost();
                EventResult::Ignored
            }
            // Release the drag capture whatever the enabled state — a disabled
            // toggle mid-drag must still let go of the pointer.
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
            // Everything below newly enters an active state or moves the value —
            // inert while disabled.
            _ if disabled => EventResult::Ignored,
            WidgetEvent::MouseEnter => {
                self.state.enter(disabled);
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                position,
            } => {
                // A press anywhere on the track jumps the thumb there and grabs
                // the pointer so the drag survives leaving the widget bounds.
                self.dragging = true;
                ctx.capture_pointer();
                let v = self.value_at_x(position.x, layout);
                self.commit(v, ctx);
                EventResult::Consumed
            }
            WidgetEvent::MouseMove { position } if self.dragging => {
                let v = self.value_at_x(position.x, layout);
                self.commit(v, ctx);
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.state.focus_gained(disabled);
                EventResult::Ignored
            }
            // Keyboard: arrows nudge by a step, Home/End jump to the ends. Only
            // while focused so a global arrow press doesn't move a stray slider.
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown),
            } if self.state.focused => {
                self.nudge(-1.0, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp),
            } if self.state.focused => {
                self.nudge(1.0, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Home),
            } if self.state.focused => {
                let min = self.min;
                self.commit(min, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::End),
            } if self.state.focused => {
                let max = self.max;
                self.commit(max, ctx);
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sindon_core::Point;
    use std::cell::Cell as StdCell;
    use std::rc::Rc;

    /// A 118px-wide layout: usable travel = 118 − 18 = 100, so thumb-centre x in
    /// [9, 109] maps linearly to [min, max].
    fn layout() -> Rect {
        Rect::new(0.0, 0.0, 118.0, THUMB_D)
    }

    fn down(x: f32) -> WidgetEvent {
        WidgetEvent::MouseDown {
            position: Point::new(x, 9.0),
            button: MouseButton::Left,
        }
    }

    fn key(named: NamedKey) -> WidgetEvent {
        WidgetEvent::KeyDown {
            key: Key::Named(named),
        }
    }

    #[test]
    fn value_at_x_maps_ends_and_middle() {
        let s = Slider::new(0.0, 100.0);
        assert_eq!(s.value_at_x(9.0, layout()), 0.0, "left end → min");
        assert_eq!(s.value_at_x(109.0, layout()), 100.0, "right end → max");
        assert!(
            (s.value_at_x(59.0, layout()) - 50.0).abs() < 0.01,
            "middle → midpoint"
        );
        // Past the ends clamps.
        assert_eq!(s.value_at_x(-50.0, layout()), 0.0);
        assert_eq!(s.value_at_x(500.0, layout()), 100.0);
    }

    #[test]
    fn snap_rounds_to_step_grid() {
        let s = Slider::new(0.0, 10.0).step(2.0);
        assert_eq!(s.snap(3.0), 4.0, "3 rounds to nearest even");
        assert_eq!(s.snap(0.9), 0.0);
        assert_eq!(s.snap(9.9), 10.0);
    }

    #[test]
    fn drag_sets_value_and_fires_handler() {
        let seen = Rc::new(StdCell::new(0.0));
        let s2 = Rc::clone(&seen);
        let mut sl = Slider::new(0.0, 100.0).on_change(move |v, _| s2.set(v));
        let mut ctx = EventContext::new();

        sl.event(&down(59.0), layout(), &mut ctx);
        assert!((sl.get() - 50.0).abs() < 0.01, "press jumps to midpoint");
        assert!((seen.get() - 50.0).abs() < 0.01, "handler saw the value");

        // A move while dragging keeps updating.
        sl.event(
            &WidgetEvent::MouseMove {
                position: Point::new(109.0, 9.0),
            },
            layout(),
            &mut ctx,
        );
        assert_eq!(sl.get(), 100.0, "drag to the right end reaches max");

        sl.event(
            &WidgetEvent::MouseUp {
                position: Point::new(109.0, 9.0),
                button: MouseButton::Left,
            },
            layout(),
            &mut ctx,
        );
        assert!(!sl.dragging, "release ends the drag");
    }

    #[test]
    fn screen_reader_steps_and_sets_the_value() {
        let mut sl = Slider::new(0.0, 10.0).step(2.0).value(4.0);
        let mut ctx = EventContext::new();

        // Stepping uses the widget's own step grid — the same nudge the arrow
        // keys take.
        let r = sl.accessibility_action(AccessAction::Increment, None, layout(), &mut ctx);
        assert_eq!(r, EventResult::Consumed);
        assert_eq!(sl.get(), 6.0, "Increment steps up by one step");
        sl.accessibility_action(AccessAction::Decrement, None, layout(), &mut ctx);
        assert_eq!(sl.get(), 4.0, "Decrement steps back down");

        // An absolute set goes through `commit`, so it snaps to the grid.
        sl.accessibility_action(AccessAction::SetValue(7.0), None, layout(), &mut ctx);
        assert_eq!(sl.get(), 8.0, "SetValue snaps to the step grid");
    }

    #[test]
    fn screen_reader_set_value_is_clamped_to_range() {
        // An AT is free to ask for anything; the widget's own clamp is what
        // keeps the value on its rails.
        let mut sl = Slider::new(0.0, 10.0).value(5.0);
        let mut ctx = EventContext::new();

        sl.accessibility_action(AccessAction::SetValue(9999.0), None, layout(), &mut ctx);
        assert_eq!(sl.get(), 10.0, "above max clamps to max");
        sl.accessibility_action(AccessAction::SetValue(-9999.0), None, layout(), &mut ctx);
        assert_eq!(sl.get(), 0.0, "below min clamps to min");
    }

    #[test]
    fn disabled_slider_refuses_screen_reader_actions() {
        let mut sl = Slider::new(0.0, 10.0).value(5.0).disabled(true);
        let mut ctx = EventContext::new();

        for action in [
            AccessAction::Increment,
            AccessAction::Decrement,
            AccessAction::SetValue(9.0),
            AccessAction::Click,
        ] {
            assert_eq!(
                sl.accessibility_action(action, None, layout(), &mut ctx),
                EventResult::Ignored,
                "{action:?} must be inert while disabled"
            );
            assert_eq!(sl.get(), 5.0, "{action:?} must not move the value");
        }
    }

    #[test]
    fn move_without_press_does_not_change() {
        let mut sl = Slider::new(0.0, 100.0).value(20.0);
        let mut ctx = EventContext::new();
        sl.event(
            &WidgetEvent::MouseMove {
                position: Point::new(109.0, 9.0),
            },
            layout(),
            &mut ctx,
        );
        assert_eq!(sl.get(), 20.0, "a hover move with no drag is inert");
    }

    #[test]
    fn arrows_and_home_end_nudge_when_focused() {
        let mut sl = Slider::new(0.0, 10.0).step(1.0).value(5.0);
        let mut ctx = EventContext::new();

        // Inert without focus.
        sl.event(&key(NamedKey::ArrowRight), layout(), &mut ctx);
        assert_eq!(sl.get(), 5.0, "arrows do nothing without focus");

        sl.event(&WidgetEvent::FocusGained, layout(), &mut ctx);
        sl.event(&key(NamedKey::ArrowRight), layout(), &mut ctx);
        assert_eq!(sl.get(), 6.0, "Right nudges up one step");
        sl.event(&key(NamedKey::ArrowDown), layout(), &mut ctx);
        assert_eq!(sl.get(), 5.0, "Down nudges down one step");

        sl.event(&key(NamedKey::End), layout(), &mut ctx);
        assert_eq!(sl.get(), 10.0, "End jumps to max");
        sl.event(&key(NamedKey::Home), layout(), &mut ctx);
        assert_eq!(sl.get(), 0.0, "Home jumps to min");

        // Clamps at the ends.
        sl.event(&key(NamedKey::ArrowLeft), layout(), &mut ctx);
        assert_eq!(sl.get(), 0.0, "Left at min stays at min");
    }

    #[test]
    fn bound_signal_is_two_way() {
        let sig = Signal::new(0.0f32);
        let mut sl = Slider::new(0.0, 100.0).bind(sig);
        let mut ctx = EventContext::new();

        sl.event(&down(109.0), layout(), &mut ctx);
        assert_eq!(sig.get(), 100.0, "drag writes the bound signal");

        sig.set(25.0);
        assert_eq!(
            sl.get(),
            25.0,
            "external signal write is the source of truth"
        );
    }

    #[test]
    fn disabled_ignores_drag_and_keys() {
        let mut sl = Slider::new(0.0, 100.0).value(30.0).disabled(true);
        let mut ctx = EventContext::new();

        sl.event(&down(109.0), layout(), &mut ctx);
        assert_eq!(sl.get(), 30.0, "disabled slider ignores a press");
        assert!(!sl.dragging, "disabled slider does not begin a drag");

        assert!(!sl.focusable(), "disabled slider is out of the Tab order");
    }
}
