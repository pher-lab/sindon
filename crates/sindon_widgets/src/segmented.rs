//! Segmented control — pick one of N options from a horizontal row of pills.
//!
//! A compact single-choice control: the options sit side by side in one bar and
//! the selected one is highlighted, the natural shape for a small mutually
//! exclusive set (view mode, sort order, a two- or three-way filter). Its taller
//! sibling is [`RadioGroup`](crate::RadioGroup) — same selection model, vertical
//! list layout.
//!
//! Like the other Phase 44 controls it carries the [`Input`](crate::Input)-style
//! binding API: an owned initial index, an optional two-way [`Signal<usize>`],
//! `on_change`, `.disabled`. The whole bar is one focusable widget (the ARIA
//! radiogroup pattern): click a segment to select it, or once focused use
//! Left/Right (or Up/Down) to move — clamped at the ends — and Home/End to jump.

use std::cell::Cell;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::interaction::{InteractionState, dim_over, step_selection};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{AccessAction, AccessChild, AccessNode, AccessRole, Color, Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::{Reactive, Signal};

/// Horizontal padding inside each segment, per side.
const SEG_PAD_X: f32 = 14.0;
/// Vertical padding above/below the label.
const SEG_PAD_Y: f32 = 8.0;
/// Inset of the selected chip from the track edge, so the highlight reads as a
/// raised pill sitting inside the groove rather than filling it edge to edge.
const CHIP_INSET: f32 = 3.0;

/// Handler for [`Segmented::on_change`]. Receives the new selected index and the
/// dispatch context.
type ChangeHandler = Box<dyn FnMut(usize, &mut EventContext)>;

/// A horizontal single-select control over a fixed set of labelled segments.
///
/// # Example (conceptual)
/// ```ignore
/// let mode = Signal::new(0usize);
/// let seg = Segmented::new(["Edit", "Preview"])
///     .bind(mode)
///     .on_change(|i, _ctx| println!("mode: {i}"));
/// ```
pub struct Segmented {
    labels: Vec<String>,
    font_size: Option<f32>,
    /// Owned mirror of the selected index. The bound [`source`](Self::source) is
    /// the source of truth on read when present.
    selected: Cell<usize>,
    source: Option<Signal<usize>>,
    on_change: Option<ChangeHandler>,
    disabled: Reactive<bool>,
    /// Hover / focus flags (see [`InteractionState`]). Selection happens on
    /// press, so the press latch goes unused.
    state: InteractionState,
    // Colours (None = read from theme each frame).
    track_color: Option<Color>,
    selected_color: Option<Color>,
    selected_label_color: Option<Color>,
    label_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl Segmented {
    /// Create a segmented control from a list of labels. The first segment is
    /// selected initially (override with [`selected`](Self::selected) /
    /// [`bind`](Self::bind)).
    pub fn new(labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            labels: labels.into_iter().map(Into::into).collect(),
            font_size: None,
            selected: Cell::new(0),
            source: None,
            on_change: None,
            disabled: Reactive::Static(false),
            state: InteractionState::default(),
            track_color: None,
            selected_color: None,
            selected_label_color: None,
            label_color: None,
            focus_ring_color: None,
        }
    }

    /// Set the initially-selected index (clamped to the option range). Ignored
    /// once a [`bind`](Self::bind) signal is attached.
    pub fn selected(self, index: usize) -> Self {
        let clamped = index.min(self.labels.len().saturating_sub(1));
        self.selected.set(clamped);
        self
    }

    /// Bind two-way to a [`Signal<usize>`]. The signal becomes the source of
    /// truth: external writes are reflected on the next paint, and every
    /// selection writes back to it (and still fires
    /// [`on_change`](Self::on_change)).
    pub fn bind(mut self, signal: Signal<usize>) -> Self {
        let clamped = signal.get().min(self.labels.len().saturating_sub(1));
        self.selected.set(clamped);
        self.source = Some(signal);
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set a callback fired when the selection changes.
    pub fn on_change(mut self, f: impl FnMut(usize, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Gate the control on a disabled state (reactive). While `true` it drops
    /// hover feedback, is skipped by Tab, ignores clicks and keys, and paints
    /// muted — faded toward the surface, kept opaque so the selected chip still
    /// covers the groove.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Override the track (groove) colour. `None` reads
    /// `theme.colors.surface_variant`.
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = Some(color);
        self
    }

    /// Override the selected-chip colour. `None` reads `theme.colors.primary`.
    pub fn selected_color(mut self, color: Color) -> Self {
        self.selected_color = Some(color);
        self
    }

    /// Override the selected segment's label colour. `None` reads
    /// `theme.colors.on_primary`.
    pub fn selected_label_color(mut self, color: Color) -> Self {
        self.selected_label_color = Some(color);
        self
    }

    /// Override the unselected segments' label colour. `None` reads
    /// `theme.colors.on_surface_variant`.
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

    /// The current selected index (reads the bound signal if attached), clamped
    /// to the option range.
    pub fn selected_index(&self) -> usize {
        let n = self.labels.len();
        if n == 0 {
            return 0;
        }
        let raw = match &self.source {
            Some(s) => s.get(),
            None => self.selected.get(),
        };
        raw.min(n - 1)
    }

    /// Whether this control currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    /// Segment index under a screen-space x, given the widget's layout.
    fn segment_at_x(&self, x: f32, layout: Rect) -> usize {
        let n = self.labels.len();
        if n == 0 {
            return 0;
        }
        let seg_w = (layout.size.width / n as f32).max(1.0);
        let rel = (x - layout.origin.x).max(0.0);
        ((rel / seg_w) as usize).min(n - 1)
    }

    /// Commit a new selection: write the mirror, the bound signal, and fire the
    /// change handler — only when the index actually changed.
    fn commit(&mut self, index: usize, ctx: &mut EventContext) {
        let n = self.labels.len();
        if n == 0 {
            return;
        }
        let index = index.min(n - 1);
        if index == self.selected_index() {
            return;
        }
        self.selected.set(index);
        if let Some(s) = &self.source {
            s.set(index);
        }
        if let Some(handler) = &mut self.on_change {
            handler(index, ctx);
        }
    }
}

impl Widget for Segmented {
    fn focusable(&self) -> bool {
        !self.disabled.get()
    }

    fn accessibility(&self) -> Option<AccessNode> {
        // The bar itself is the tab list; each segment is a `Tab` child (see
        // `accessibility_children`). Its name stays the selected label so an AT
        // that summarises the group still announces the active choice.
        let mut node = AccessNode::new(AccessRole::TabList).disabled(self.disabled.get());
        if let Some(label) = self.labels.get(self.selected_index()) {
            node = node.name(label.clone());
        }
        Some(node)
    }

    fn accessibility_children(&self, layout: Rect) -> Vec<AccessChild> {
        let n = self.labels.len();
        if n == 0 {
            return Vec::new();
        }
        // One node per segment, boxed the way `paint` boxes them, so the AT's
        // highlight lands on the segment the user hears.
        let disabled = self.disabled.get();
        let selected = self.selected_index();
        let seg_w = layout.size.width / n as f32;
        self.labels
            .iter()
            .enumerate()
            .map(|(i, label)| AccessChild {
                node: AccessNode::new(AccessRole::Tab)
                    .name(label.clone())
                    .selected(i == selected)
                    .disabled(disabled),
                bounds: Rect::new(
                    layout.origin.x + i as f32 * seg_w,
                    layout.origin.y,
                    seg_w,
                    layout.size.height,
                ),
            })
            .collect()
    }

    fn accessibility_action(
        &mut self,
        action: AccessAction,
        option: Option<usize>,
        _layout: Rect,
        ctx: &mut EventContext,
    ) -> EventResult {
        // Only a segment is clickable — `TabList` is not an activatable role,
        // so the bar's own node never advertises the action and `option` is
        // always the real target here.
        if action != AccessAction::Click || self.disabled.get() {
            return EventResult::Ignored;
        }
        let Some(index) = option.filter(|&i| i < self.labels.len()) else {
            return EventResult::Ignored;
        };
        self.commit(index, ctx);
        EventResult::Consumed
    }

    fn style(&self) -> FlexStyle {
        // Measured leaf — no `min_size` here (see `measure` / the Button note).
        FlexStyle::new()
    }

    fn measure(&self, _available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        // All segments share the widest label's width so the bar reads as even.
        let mut widest = 0.0f32;
        for label in &self.labels {
            let (label_w, _) =
                ctx.text_engine
                    .measure_text(label, font_size, font_size * 1.2, None);
            widest = widest.max(label_w);
        }
        let seg_w = widest + 2.0 * SEG_PAD_X;
        let w = (seg_w * self.labels.len().max(1) as f32).ceil();
        let h = (font_size + 2.0 * SEG_PAD_Y).ceil();
        Some(Size::new(w.max(1.0), h))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let n = self.labels.len();
        if n == 0 {
            return;
        }
        let disabled = self.disabled.get();
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let selected = self.selected_index();

        let radius = ctx.theme.shape.radius_md;
        let surface = ctx.theme.colors.surface;
        let mut track = self.track_color.unwrap_or(ctx.theme.colors.surface_variant);
        let mut chip = self.selected_color.unwrap_or(ctx.theme.colors.primary);
        if disabled {
            track = dim_over(track, surface);
            chip = dim_over(chip, surface);
        }

        // Groove behind all segments, plus a hairline outline so the control's
        // bounds read even when the track colour is close to the surface behind
        // it (a dark panel where `surface_variant` ≈ the backdrop).
        ctx.fill_rect_rounded(layout, track, radius);
        let mut outline = ctx.theme.colors.outline;
        if disabled {
            outline = dim_over(outline, surface);
        }
        ctx.stroke_rect_rounded(layout, outline, radius, 1.0);

        let seg_w = layout.size.width / n as f32;

        // Selected chip, inset so it reads as raised inside the groove.
        let chip_x = layout.origin.x + selected as f32 * seg_w + CHIP_INSET;
        let chip_rect = Rect::new(
            chip_x,
            layout.origin.y + CHIP_INSET,
            (seg_w - 2.0 * CHIP_INSET).max(0.0),
            (layout.size.height - 2.0 * CHIP_INSET).max(0.0),
        );
        ctx.fill_rect_rounded(chip_rect, chip, (radius - CHIP_INSET).max(0.0));

        // Per-segment centred label.
        let sel_label = self
            .selected_label_color
            .unwrap_or(ctx.theme.colors.on_primary);
        let unsel_label = self
            .label_color
            .unwrap_or(ctx.theme.colors.on_surface_variant);
        for (i, label) in self.labels.iter().enumerate() {
            let seg_x = layout.origin.x + i as f32 * seg_w;
            let shaped = ctx
                .text_engine
                .shape_text(label, font_size, font_size * 1.2, Some(seg_w));
            let text_x = seg_x + (seg_w - shaped.width) / 2.0;
            let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
            let mut color = if i == selected {
                sel_label
            } else {
                unsel_label
            };
            if disabled {
                color = dim_over(color, surface);
            }
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

        if self.state.focused && !disabled && ctx.focus_visible() {
            ctx.paint_focus_ring(layout, self.focus_ring_color, radius);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        let disabled = self.disabled.get();
        let n = self.labels.len();
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
            // Everything below newly enters an active state or selects — inert
            // while disabled.
            _ if disabled => EventResult::Ignored,
            WidgetEvent::MouseEnter => {
                self.state.enter(disabled);
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                position,
            } => {
                let idx = self.segment_at_x(position.x, layout);
                self.commit(idx, ctx);
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.state.focus_gained(disabled);
                EventResult::Ignored
            }
            // Arrows move selection (clamped, no wrap); Home/End jump to the
            // ends. Only while focused.
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp),
            } if self.state.focused => {
                let idx = step_selection(self.selected_index(), n, -1);
                self.commit(idx, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown),
            } if self.state.focused => {
                let idx = step_selection(self.selected_index(), n, 1);
                self.commit(idx, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Home),
            } if self.state.focused => {
                self.commit(0, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::End),
            } if self.state.focused => {
                self.commit(n.saturating_sub(1), ctx);
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

    /// A 300px-wide, 3-segment layout: each segment is 100px, so x in [0,100)
    /// → 0, [100,200) → 1, [200,300) → 2.
    fn layout() -> Rect {
        Rect::new(0.0, 0.0, 300.0, 34.0)
    }

    fn seg() -> Segmented {
        Segmented::new(["A", "B", "C"])
    }

    fn down(x: f32) -> WidgetEvent {
        WidgetEvent::MouseDown {
            position: Point::new(x, 10.0),
            button: MouseButton::Left,
        }
    }

    fn key(named: NamedKey) -> WidgetEvent {
        WidgetEvent::KeyDown {
            key: Key::Named(named),
        }
    }

    #[test]
    fn segment_at_x_maps_thirds() {
        let s = seg();
        assert_eq!(s.segment_at_x(50.0, layout()), 0);
        assert_eq!(s.segment_at_x(150.0, layout()), 1);
        assert_eq!(s.segment_at_x(250.0, layout()), 2);
        // Past the right edge clamps to the last segment.
        assert_eq!(s.segment_at_x(999.0, layout()), 2);
    }

    #[test]
    fn click_selects_segment_and_fires_handler() {
        let seen = Rc::new(StdCell::new(usize::MAX));
        let s2 = Rc::clone(&seen);
        let mut s = seg().on_change(move |i, _| s2.set(i));
        let mut ctx = EventContext::new();

        s.event(&down(250.0), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 2, "click on third segment selects it");
        assert_eq!(seen.get(), 2, "handler saw the index");

        // Re-clicking the same segment does not re-fire.
        seen.set(usize::MAX);
        s.event(&down(260.0), layout(), &mut ctx);
        assert_eq!(seen.get(), usize::MAX, "no change → no on_change");
    }

    #[test]
    fn arrows_and_home_end_move_selection_when_focused() {
        let mut s = seg().selected(1);
        let mut ctx = EventContext::new();

        // Inert without focus.
        s.event(&key(NamedKey::ArrowRight), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 1, "arrows do nothing without focus");

        s.event(&WidgetEvent::FocusGained, layout(), &mut ctx);
        s.event(&key(NamedKey::ArrowRight), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 2, "Right moves to next");
        s.event(&key(NamedKey::ArrowRight), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 2, "clamps at the last segment");
        s.event(&key(NamedKey::Home), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 0, "Home selects the first");
        s.event(&key(NamedKey::End), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 2, "End selects the last");
    }

    #[test]
    fn bound_signal_is_two_way() {
        let sig = Signal::new(0usize);
        let mut s = seg().bind(sig);
        let mut ctx = EventContext::new();

        s.event(&down(150.0), layout(), &mut ctx);
        assert_eq!(sig.get(), 1, "click writes the bound signal");

        sig.set(2);
        assert_eq!(
            s.selected_index(),
            2,
            "external write is the source of truth"
        );
    }

    #[test]
    fn every_segment_is_exposed_as_its_own_node() {
        // The point of the per-option children: a screen reader can enumerate
        // the choices, not just hear the selected one.
        let s = seg().selected(1);
        let children = s.accessibility_children(layout());
        assert_eq!(children.len(), 3, "one node per segment");

        for (i, child) in children.iter().enumerate() {
            assert_eq!(child.node.role, AccessRole::Tab);
            assert_eq!(child.node.name.as_deref(), Some(["A", "B", "C"][i]));
            assert_eq!(
                child.node.selected,
                Some(i == 1),
                "only the selected segment reports selected"
            );
            // Bounds match what `paint` draws: even thirds of a 300px bar.
            assert_eq!(child.bounds.origin.x, i as f32 * 100.0);
            assert_eq!(child.bounds.size.width, 100.0);
        }
    }

    #[test]
    fn screen_reader_click_selects_the_targeted_segment() {
        let seen = Rc::new(StdCell::new(usize::MAX));
        let s2 = Rc::clone(&seen);
        let mut s = seg().on_change(move |i, _| s2.set(i));
        let mut ctx = EventContext::new();

        let r = s.accessibility_action(AccessAction::Click, Some(2), layout(), &mut ctx);
        assert_eq!(r, EventResult::Consumed);
        assert_eq!(s.selected_index(), 2, "the AT's target segment is selected");
        assert_eq!(seen.get(), 2, "handler fired as for a mouse click");
    }

    #[test]
    fn screen_reader_click_needs_a_real_segment() {
        let mut s = seg().selected(1);
        let mut ctx = EventContext::new();

        // The bar itself is a TabList (not activatable), so a click with no
        // option is not a selection.
        assert_eq!(
            s.accessibility_action(AccessAction::Click, None, layout(), &mut ctx),
            EventResult::Ignored,
        );
        // A stale id from a snapshot taken when the bar had more segments.
        assert_eq!(
            s.accessibility_action(AccessAction::Click, Some(9), layout(), &mut ctx),
            EventResult::Ignored,
        );
        assert_eq!(s.selected_index(), 1, "selection untouched");
    }

    #[test]
    fn disabled_segmented_refuses_the_screen_reader_click() {
        let mut s = seg().selected(1).disabled(true);
        let mut ctx = EventContext::new();

        let r = s.accessibility_action(AccessAction::Click, Some(2), layout(), &mut ctx);
        assert_eq!(r, EventResult::Ignored);
        assert_eq!(s.selected_index(), 1, "state unchanged");
    }

    #[test]
    fn disabled_ignores_clicks_and_keys() {
        let mut s = seg().selected(1).disabled(true);
        let mut ctx = EventContext::new();

        s.event(&down(250.0), layout(), &mut ctx);
        assert_eq!(s.selected_index(), 1, "disabled control ignores a click");
        assert!(!s.focusable(), "disabled control is out of the Tab order");
    }
}
