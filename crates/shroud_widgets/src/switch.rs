//! Switch widget — a boolean toggle rendered as a sliding pill.
//!
//! The non-text sibling of [`Checkbox`](crate::Checkbox): where a checkbox
//! reads as "tick this option", a switch reads as "this setting is on/off" and
//! is the natural control for a settings row. Like [`Input`](crate::Input) it
//! carries the full binding API — an owned initial value, an optional two-way
//! [`Signal<bool>`] the surrounding screen drives, and an `on_change` callback
//! — so a settings signal can own the truth while the widget stays a thin view.
//!
//! Toggles on press (matching [`Checkbox`]) and on Space while focused; Enter is
//! deliberately left alone for form-submit. The knob slides and the track colour
//! cross-fades via an [`Animated`], the same mechanism [`Button`](crate::Button)
//! uses for its hover fade — and because that read happens in `paint`, an
//! external `signal.set(...)` with no event still animates the knob across.

use std::cell::Cell;
use std::time::Duration;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::interaction::{InteractionState, dim_over};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{AccessNode, AccessRole, Color, Lerp, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::{Animated, Easing, Reactive, Signal};

/// Knob-slide / track-colour transition duration. Short enough to feel like a
/// direct toggle, long enough to read as a slide rather than a jump.
const KNOB_TRANSITION: Duration = Duration::from_millis(140);

/// Handler for [`Switch::on_change`]. Receives the new state and the dispatch
/// context for queuing tree mutations.
type ChangeHandler = Box<dyn FnMut(bool, &mut EventContext)>;

/// A boolean on/off toggle with an optional trailing label.
///
/// # Example (conceptual)
/// ```ignore
/// let dark_mode = Signal::new(false);
/// let sw = Switch::new()
///     .bind(dark_mode)
///     .label("Dark mode")
///     .on_change(|on, _ctx| println!("dark mode: {on}"));
/// ```
pub struct Switch {
    /// Owned mirror of the value. When a [`source`](Self::source) signal is
    /// bound it is the source of truth on read; this mirror keeps an unbound
    /// switch stateful and lets a bound one paint without a signal round-trip.
    value: Cell<bool>,
    /// Optional bound signal. Read fresh each paint (so external
    /// `signal.set(...)` shows up next frame) and written on every user toggle.
    source: Option<Signal<bool>>,
    label: String,
    font_size: Option<f32>,
    on_change: Option<ChangeHandler>,
    /// Gate on a disabled state (reactive), mirroring [`Button::disabled`].
    disabled: Reactive<bool>,
    /// Hover / focus flags. A switch toggles on press, so the `pressed` latch
    /// goes unused — the shared invariants (clear-even-while-disabled) still
    /// apply. See [`InteractionState`].
    state: InteractionState,
    /// Knob position 0.0 (off) → 1.0 (on), eased. Interior-mutable so `paint`
    /// (`&self`) can retarget it when the value changes, including an external
    /// signal write that arrives with no event.
    knob: Animated<f32>,
    /// Last value the knob animator observed. `None` until the first paint
    /// primes it — that first observation *snaps* (see [`Animated::snap`]) so
    /// the initial state doesn't slide in from off.
    knob_seen: Cell<Option<bool>>,
    // Colours (None = read from theme each frame).
    track_on_color: Option<Color>,
    track_off_color: Option<Color>,
    knob_color: Option<Color>,
    label_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl Switch {
    /// Create a switch, initially off, with no label.
    pub fn new() -> Self {
        Self {
            value: Cell::new(false),
            source: None,
            label: String::new(),
            font_size: None,
            on_change: None,
            disabled: Reactive::Static(false),
            state: InteractionState::default(),
            knob: Animated::new(0.0, KNOB_TRANSITION, Easing::EaseInOut),
            knob_seen: Cell::new(None),
            track_on_color: None,
            track_off_color: None,
            knob_color: None,
            label_color: None,
            focus_ring_color: None,
        }
    }

    /// Set the initial on/off state. Ignored once a [`bind`](Self::bind) signal
    /// is attached — the signal then owns the value.
    pub fn on(self, on: bool) -> Self {
        self.value.set(on);
        self
    }

    /// Bind the switch two-way to a [`Signal<bool>`]. The signal becomes the
    /// source of truth: external `signal.set(...)` is reflected on the next
    /// paint, and every user toggle writes back to it (and still fires
    /// [`on_change`](Self::on_change)).
    pub fn bind(mut self, signal: Signal<bool>) -> Self {
        self.value.set(signal.get());
        self.source = Some(signal);
        self
    }

    /// Set the trailing label text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Set the font size (label and control scale together).
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set a callback fired when the state changes (by click or Space). Receives
    /// the new state and the [`EventContext`].
    pub fn on_change(mut self, f: impl FnMut(bool, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Gate the switch on a disabled state (reactive). While `true` it drops
    /// hover feedback, is skipped by Tab, and does not toggle — the disabled
    /// track/knob/label paint muted (faded toward the surface, kept opaque so
    /// the knob still covers the track). Accepts a literal `bool` or a
    /// signal-backed source.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Override the track colour when on. `None` reads `theme.colors.primary`.
    pub fn track_on_color(mut self, color: Color) -> Self {
        self.track_on_color = Some(color);
        self
    }

    /// Override the track colour when off. `None` reads `theme.colors.outline`.
    pub fn track_off_color(mut self, color: Color) -> Self {
        self.track_off_color = Some(color);
        self
    }

    /// Override the sliding knob colour. `None` reads `theme.colors.on_primary`.
    pub fn knob_color(mut self, color: Color) -> Self {
        self.knob_color = Some(color);
        self
    }

    /// Override the label text colour. `None` reads `theme.colors.on_background`.
    pub fn label_color(mut self, color: Color) -> Self {
        self.label_color = Some(color);
        self
    }

    /// Override the keyboard-focus ring colour. `None` reads
    /// `theme.focus.ring_color`.
    pub fn focus_ring_color(mut self, color: Color) -> Self {
        self.focus_ring_color = Some(color);
        self
    }

    /// The current on/off state (reads the bound signal if one is attached).
    pub fn is_on(&self) -> bool {
        match &self.source {
            Some(s) => s.get(),
            None => self.value.get(),
        }
    }

    /// Whether this switch currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    /// Write a new value to the mirror, the bound signal (if any), and fire the
    /// change handler. The single commit path shared by click and Space.
    fn commit(&mut self, on: bool, ctx: &mut EventContext) {
        self.value.set(on);
        if let Some(s) = &self.source {
            s.set(on);
        }
        if let Some(handler) = &mut self.on_change {
            handler(on, ctx);
        }
    }

    /// Flip the current state.
    fn toggle(&mut self, ctx: &mut EventContext) {
        let next = !self.is_on();
        self.commit(next, ctx);
    }

    /// Pill height for a given font size.
    fn track_height(&self, font_size: f32) -> f32 {
        font_size + 4.0
    }

    /// Pill width for a given track height — a 1.8:1 pill leaves room for the
    /// knob to travel a clearly-readable distance.
    fn track_width(&self, track_h: f32) -> f32 {
        track_h * 1.8
    }
}

impl Default for Switch {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Switch {
    fn focusable(&self) -> bool {
        !self.disabled.get()
    }

    fn accessibility(&self) -> Option<AccessNode> {
        let mut node = AccessNode::new(AccessRole::Switch)
            .checked(self.is_on())
            .disabled(self.disabled.get());
        if !self.label.is_empty() {
            node = node.name(self.label.clone());
        }
        Some(node)
    }

    fn style(&self) -> FlexStyle {
        // Measured leaf (see `measure`): no `min_size` here, or an ancestor
        // that hugs its content over-counts height. Same invariant as `Button`.
        FlexStyle::new().row().gap(8.0).align_center()
    }

    fn measure(&self, _available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let track_h = self.track_height(font_size);
        let track_w = self.track_width(track_h);
        let mut w = track_w;
        let mut h = track_h.max(font_size);
        if !self.label.is_empty() {
            let line_height = font_size * 1.2;
            let shaped = ctx
                .text_engine
                .shape_text(&self.label, font_size, line_height, None);
            w += 8.0 + shaped.width;
            h = h.max(shaped.height);
        }
        Some(Size::new(w.ceil(), h.ceil()))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let disabled = self.disabled.get();
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let on = self.is_on();

        // Prime / retarget the knob animation on value change. The first paint
        // snaps (no slide-in); later changes — including external signal
        // writes with no event — ease.
        match self.knob_seen.get() {
            None => {
                self.knob.snap(if on { 1.0 } else { 0.0 });
                self.knob_seen.set(Some(on));
            }
            Some(prev) if prev != on => {
                self.knob.set(if on { 1.0 } else { 0.0 });
                self.knob_seen.set(Some(on));
            }
            _ => {}
        }
        let t = self.knob.get().clamp(0.0, 1.0);

        // Track geometry — vertically centred in the row.
        let track_h = self.track_height(font_size);
        let track_w = self.track_width(track_h);
        let track_x = layout.origin.x;
        let track_y = layout.origin.y + (layout.size.height - track_h) / 2.0;
        let track_rect = Rect::new(track_x, track_y, track_w, track_h);
        let radius = track_h / 2.0;

        let on_col = self.track_on_color.unwrap_or(ctx.theme.colors.primary);
        let off_col = self.track_off_color.unwrap_or(ctx.theme.colors.outline);
        let mut track = off_col.lerp(&on_col, t);
        let mut knob_col = self.knob_color.unwrap_or(ctx.theme.colors.on_primary);
        if disabled {
            let surface = ctx.theme.colors.surface;
            track = dim_over(track, surface);
            knob_col = dim_over(knob_col, surface);
        }
        ctx.fill_rect_rounded(track_rect, track, radius);

        // Knob — a disc inset from the track edge, sliding across on `t`.
        let pad = track_h * 0.12;
        let knob_d = track_h - pad * 2.0;
        let travel = track_w - knob_d - pad * 2.0;
        let knob_x = track_x + pad + travel * t;
        let knob_y = track_y + pad;
        ctx.fill_rect_rounded(
            Rect::new(knob_x, knob_y, knob_d, knob_d),
            knob_col,
            knob_d / 2.0,
        );

        // Label.
        if !self.label.is_empty() {
            let label_x = track_x + track_w + 8.0;
            let label_y = layout.origin.y + (layout.size.height - font_size) / 2.0;
            let max_width = layout.size.width - track_w - 16.0;
            let base = self.label_color.unwrap_or(ctx.theme.colors.on_background);
            let label_col = if disabled {
                dim_over(base, ctx.theme.colors.surface)
            } else {
                base
            };
            if max_width > 0.0 {
                let shaped = ctx.text_engine.shape_text(
                    &self.label,
                    font_size,
                    font_size * 1.2,
                    Some(max_width),
                );
                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            label_x as i32 + glyph.x,
                            label_y as i32 + glyph.y,
                            image,
                            label_col,
                            glyph.cache_key,
                        );
                    }
                }
            }
        }

        // Focus ring follows the whole row (the entire row is the hit target).
        if self.state.focused && !disabled && ctx.focus_visible() {
            ctx.paint_focus_ring(layout, self.focus_ring_color, 0.0);
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
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
            // Everything below newly enters an active state or toggles — inert
            // while disabled.
            _ if disabled => EventResult::Ignored,
            WidgetEvent::MouseEnter => {
                self.state.enter(disabled);
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.toggle(ctx);
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.state.focus_gained(disabled);
                EventResult::Ignored
            }
            // Space toggles when focused — browser parity. Enter is left for the
            // surrounding screen (form submit), matching `Checkbox`.
            WidgetEvent::CharInput { ch: ' ' } if self.state.focused => {
                self.toggle(ctx);
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroud_core::Point;
    use std::cell::Cell as StdCell;
    use std::rc::Rc;

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 120.0, 28.0)
    }

    fn down() -> WidgetEvent {
        WidgetEvent::MouseDown {
            position: Point::new(5.0, 5.0),
            button: MouseButton::Left,
        }
    }

    #[test]
    fn click_toggles_and_fires_handler() {
        let seen = Rc::new(StdCell::new(None));
        let s2 = Rc::clone(&seen);
        let mut sw = Switch::new().on_change(move |on, _| s2.set(Some(on)));
        let mut ctx = EventContext::new();

        sw.event(&down(), rect(), &mut ctx);
        assert!(sw.is_on(), "first click turns the switch on");
        assert_eq!(seen.get(), Some(true), "handler saw the new state");

        sw.event(&down(), rect(), &mut ctx);
        assert!(!sw.is_on(), "second click turns it off");
        assert_eq!(seen.get(), Some(false));
    }

    #[test]
    fn bound_signal_is_two_way() {
        let sig = Signal::new(false);
        let mut sw = Switch::new().bind(sig);
        let mut ctx = EventContext::new();

        // User toggle writes back to the signal.
        sw.event(&down(), rect(), &mut ctx);
        assert!(sig.get(), "toggle writes true to the bound signal");

        // External signal write is reflected on read.
        sig.set(false);
        assert!(!sw.is_on(), "external signal write is the source of truth");
    }

    #[test]
    fn space_toggles_only_when_focused() {
        let mut sw = Switch::new();
        let mut ctx = EventContext::new();

        let space = WidgetEvent::CharInput { ch: ' ' };
        sw.event(&space, rect(), &mut ctx);
        assert!(!sw.is_on(), "Space is inert without focus");

        sw.event(&WidgetEvent::FocusGained, rect(), &mut ctx);
        sw.event(&space, rect(), &mut ctx);
        assert!(sw.is_on(), "Space toggles once focused");
    }

    #[test]
    fn disabled_blocks_toggle_but_clears_hover() {
        let mut sw = Switch::new().disabled(true);
        let mut ctx = EventContext::new();

        sw.event(&WidgetEvent::MouseEnter, rect(), &mut ctx);
        assert!(!sw.state.hovered, "disabled switch does not hover");

        sw.event(&down(), rect(), &mut ctx);
        assert!(!sw.is_on(), "disabled switch does not toggle");

        // Even if a hover were somehow latched, MouseLeave must clear it.
        sw.state.hovered = true;
        sw.event(&WidgetEvent::MouseLeave, rect(), &mut ctx);
        assert!(
            !sw.state.hovered,
            "MouseLeave clears hover even while disabled"
        );
    }

    #[test]
    fn not_focusable_while_disabled() {
        assert!(Switch::new().focusable(), "enabled switch is focusable");
        assert!(
            !Switch::new().disabled(true).focusable(),
            "disabled switch drops out of Tab order"
        );
    }
}
