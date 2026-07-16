//! Radio group — pick one of N options from a vertical list of ring+dot rows.
//!
//! The tall sibling of [`Segmented`](crate::Segmented): the same single-select
//! model, but laid out as a vertical list where each option is a classic radio
//! (an outer ring that fills with a dot when chosen) beside its label — the
//! natural shape when the options are wordy or there are more than a handful.
//!
//! It is a *composite* widget: it owns its option list and renders every row
//! itself, the same shape as [`Dropdown`](crate::Dropdown) owning its items.
//! The whole group is one focusable widget (the ARIA radiogroup pattern): click
//! a row to select it, or once focused use Up/Down (or Left/Right, clamped, no
//! wrap) and Home/End. Carries the [`Input`](crate::Input)-style binding API:
//! owned initial index, optional two-way [`Signal<usize>`], `on_change`,
//! `.disabled`.

use std::cell::Cell;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::interaction::{InteractionState, dim_over, step_selection};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{AccessAction, AccessChild, AccessNode, AccessRole, Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::{Reactive, Signal};

/// Vertical padding above/below each row's label.
const ROW_PAD_Y: f32 = 7.0;
/// Gap between a radio's ring and its label.
const RING_GAP: f32 = 10.0;
/// Ring outline thickness.
const RING_STROKE: f32 = 1.5;

/// Handler for [`RadioGroup::on_change`]. Receives the new selected index and
/// the dispatch context.
type ChangeHandler = Box<dyn FnMut(usize, &mut EventContext)>;

/// A vertical single-select control: a list of labelled radio rows.
///
/// # Example (conceptual)
/// ```ignore
/// let theme = Signal::new(0usize);
/// let rg = RadioGroup::new(["System", "Light", "Dark"])
///     .bind(theme)
///     .on_change(|i, _ctx| println!("theme: {i}"));
/// ```
pub struct RadioGroup {
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
    selected_color: Option<Color>,
    ring_color: Option<Color>,
    label_color: Option<Color>,
    focus_ring_color: Option<Color>,
}

impl RadioGroup {
    /// Create a radio group from a list of labels. The first option is selected
    /// initially (override with [`selected`](Self::selected) /
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
            selected_color: None,
            ring_color: None,
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

    /// Set the font size (rings and labels scale together).
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set a callback fired when the selection changes.
    pub fn on_change(mut self, f: impl FnMut(usize, &mut EventContext) + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Gate the group on a disabled state (reactive). While `true` it drops
    /// hover feedback, is skipped by Tab, ignores clicks and keys, and paints
    /// muted — faded toward the surface, kept opaque so the dot still covers the
    /// ring rather than turning to glass.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Override the selected radio's ring + dot colour. `None` reads
    /// `theme.colors.primary`.
    pub fn selected_color(mut self, color: Color) -> Self {
        self.selected_color = Some(color);
        self
    }

    /// Override the unselected radios' ring colour. `None` reads
    /// `theme.colors.input_border`.
    pub fn ring_color(mut self, color: Color) -> Self {
        self.ring_color = Some(color);
        self
    }

    /// Override the label colour. `None` reads `theme.colors.on_background`.
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

    /// Whether this group currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    /// Row index under a screen-space y, given the widget's layout.
    fn row_at_y(&self, y: f32, layout: Rect) -> usize {
        let n = self.labels.len();
        if n == 0 {
            return 0;
        }
        let row_h = (layout.size.height / n as f32).max(1.0);
        let rel = (y - layout.origin.y).max(0.0);
        ((rel / row_h) as usize).min(n - 1)
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

impl Widget for RadioGroup {
    fn focusable(&self) -> bool {
        !self.disabled.get()
    }

    fn accessibility(&self) -> Option<AccessNode> {
        // A real radio group now that its options are individually exposed as
        // `RadioButton` children (the MVP borrowed `TabList` when the group was
        // a single opaque node). Its name stays the selected label so an AT
        // that summarises the group still announces the active choice.
        let mut node = AccessNode::new(AccessRole::RadioGroup).disabled(self.disabled.get());
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
        // One node per row, boxed the way `paint` (and `row_at_y`) box them.
        let disabled = self.disabled.get();
        let selected = self.selected_index();
        let row_h = layout.size.height / n as f32;
        self.labels
            .iter()
            .enumerate()
            .map(|(i, label)| AccessChild {
                node: AccessNode::new(AccessRole::RadioButton)
                    .name(label.clone())
                    .selected(i == selected)
                    .disabled(disabled),
                bounds: Rect::new(
                    layout.origin.x,
                    layout.origin.y + i as f32 * row_h,
                    layout.size.width,
                    row_h,
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
        // Only a row is clickable — `RadioGroup` is not an activatable role, so
        // the group's own node never advertises the action.
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
        let ring_d = font_size + 2.0;
        let row_h = font_size + 2.0 * ROW_PAD_Y;
        let mut widest = 0.0f32;
        for label in &self.labels {
            let shaped = ctx
                .text_engine
                .shape_text(label, font_size, font_size * 1.2, None);
            widest = widest.max(shaped.width);
        }
        let w = (ring_d + RING_GAP + widest).ceil().max(1.0);
        let h = (row_h * self.labels.len().max(1) as f32).ceil();
        Some(Size::new(w, h))
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

        let ring_d = font_size + 2.0;
        let ring_r = ring_d / 2.0;
        let dot_d = ring_d * 0.45;
        let row_h = layout.size.height / n as f32;

        let surface = ctx.theme.colors.surface;
        let sel_col = self.selected_color.unwrap_or(ctx.theme.colors.primary);
        let ring_col = self.ring_color.unwrap_or(ctx.theme.colors.input_border);
        let label_base = self.label_color.unwrap_or(ctx.theme.colors.on_background);

        for (i, label) in self.labels.iter().enumerate() {
            let row_cy = layout.origin.y + (i as f32 + 0.5) * row_h;
            let is_sel = i == selected;

            // Ring (outer circle) — accented when selected.
            let ring_x = layout.origin.x;
            let ring_rect = Rect::new(ring_x, row_cy - ring_r, ring_d, ring_d);
            let mut this_ring = if is_sel { sel_col } else { ring_col };
            if disabled {
                this_ring = dim_over(this_ring, surface);
            }
            ctx.stroke_rect_rounded(ring_rect, this_ring, ring_r, RING_STROKE);

            // Inner dot when selected.
            if is_sel {
                let mut dot = sel_col;
                if disabled {
                    dot = dim_over(dot, surface);
                }
                let dot_x = ring_x + (ring_d - dot_d) / 2.0;
                ctx.fill_rect_rounded(
                    Rect::new(dot_x, row_cy - dot_d / 2.0, dot_d, dot_d),
                    dot,
                    dot_d / 2.0,
                );
            }

            // Label.
            let label_x = ring_x + ring_d + RING_GAP;
            let max_width = layout.size.width - ring_d - RING_GAP;
            let mut color = label_base;
            if disabled {
                color = dim_over(color, surface);
            }
            if max_width > 0.0 {
                let shaped =
                    ctx.text_engine
                        .shape_text(label, font_size, font_size * 1.2, Some(max_width));
                let text_y = row_cy - shaped.height / 2.0;
                for glyph in &shaped.glyphs {
                    if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                        ctx.draw_glyph(
                            label_x as i32 + glyph.x,
                            text_y as i32 + glyph.y,
                            image,
                            color,
                            glyph.cache_key,
                        );
                    }
                }
            }
        }

        // Focus ring hugs the selected row (the radio that holds selection).
        if self.state.focused && !disabled && ctx.focus_visible() {
            let row_y = layout.origin.y + selected as f32 * row_h;
            let row_rect = Rect::new(layout.origin.x, row_y, layout.size.width, row_h);
            ctx.paint_focus_ring(row_rect, self.focus_ring_color, 0.0);
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
                let idx = self.row_at_y(position.y, layout);
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
                key: Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft),
            } if self.state.focused => {
                let idx = step_selection(self.selected_index(), n, -1);
                self.commit(idx, ctx);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight),
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
    use shroud_core::Point;
    use std::cell::Cell as StdCell;
    use std::rc::Rc;

    /// A 200x90 layout over 3 rows: each row is 30px tall, so y in [0,30) → 0,
    /// [30,60) → 1, [60,90) → 2.
    fn layout() -> Rect {
        Rect::new(0.0, 0.0, 200.0, 90.0)
    }

    fn rg() -> RadioGroup {
        RadioGroup::new(["System", "Light", "Dark"])
    }

    fn down(y: f32) -> WidgetEvent {
        WidgetEvent::MouseDown {
            position: Point::new(20.0, y),
            button: MouseButton::Left,
        }
    }

    fn key(named: NamedKey) -> WidgetEvent {
        WidgetEvent::KeyDown {
            key: Key::Named(named),
        }
    }

    #[test]
    fn row_at_y_maps_thirds() {
        let r = rg();
        assert_eq!(r.row_at_y(10.0, layout()), 0);
        assert_eq!(r.row_at_y(45.0, layout()), 1);
        assert_eq!(r.row_at_y(75.0, layout()), 2);
        // Past the bottom clamps to the last row.
        assert_eq!(r.row_at_y(999.0, layout()), 2);
    }

    #[test]
    fn click_selects_row_and_fires_handler() {
        let seen = Rc::new(StdCell::new(usize::MAX));
        let s2 = Rc::clone(&seen);
        let mut r = rg().on_change(move |i, _| s2.set(i));
        let mut ctx = EventContext::new();

        r.event(&down(75.0), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 2, "click on third row selects it");
        assert_eq!(seen.get(), 2, "handler saw the index");

        // Re-clicking the same row does not re-fire.
        seen.set(usize::MAX);
        r.event(&down(80.0), layout(), &mut ctx);
        assert_eq!(seen.get(), usize::MAX, "no change → no on_change");
    }

    #[test]
    fn arrows_and_home_end_move_selection_when_focused() {
        let mut r = rg().selected(1);
        let mut ctx = EventContext::new();

        // Inert without focus.
        r.event(&key(NamedKey::ArrowDown), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 1, "arrows do nothing without focus");

        r.event(&WidgetEvent::FocusGained, layout(), &mut ctx);
        r.event(&key(NamedKey::ArrowDown), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 2, "Down moves to next");
        r.event(&key(NamedKey::ArrowDown), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 2, "clamps at the last row");
        r.event(&key(NamedKey::ArrowUp), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 1, "Up moves back");
        r.event(&key(NamedKey::Home), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 0, "Home selects the first");
        r.event(&key(NamedKey::End), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 2, "End selects the last");
    }

    #[test]
    fn bound_signal_is_two_way() {
        let sig = Signal::new(0usize);
        let mut r = rg().bind(sig);
        let mut ctx = EventContext::new();

        r.event(&down(45.0), layout(), &mut ctx);
        assert_eq!(sig.get(), 1, "click writes the bound signal");

        sig.set(2);
        assert_eq!(
            r.selected_index(),
            2,
            "external write is the source of truth"
        );
    }

    #[test]
    fn disabled_ignores_clicks_and_keys() {
        let mut r = rg().selected(1).disabled(true);
        let mut ctx = EventContext::new();

        r.event(&down(75.0), layout(), &mut ctx);
        assert_eq!(r.selected_index(), 1, "disabled group ignores a click");
        assert!(!r.focusable(), "disabled group is out of the Tab order");
    }

    #[test]
    fn every_option_is_exposed_as_its_own_radio_button() {
        let r = rg().selected(2);
        let children = r.accessibility_children(layout());
        assert_eq!(children.len(), 3, "one node per row");

        for (i, child) in children.iter().enumerate() {
            assert_eq!(child.node.role, AccessRole::RadioButton);
            assert_eq!(
                child.node.name.as_deref(),
                Some(["System", "Light", "Dark"][i])
            );
            assert_eq!(child.node.selected, Some(i == 2));
            // Bounds match `row_at_y`: even 30px rows down a 90px group.
            assert_eq!(child.bounds.origin.y, i as f32 * 30.0);
            assert_eq!(child.bounds.size.height, 30.0);
        }
    }

    #[test]
    fn group_is_a_radio_group_named_for_its_selection() {
        let node = rg().selected(1).accessibility().expect("group has a node");
        assert_eq!(
            node.role,
            AccessRole::RadioGroup,
            "a group of radio buttons, not a tab list"
        );
        assert_eq!(node.name.as_deref(), Some("Light"));
    }

    #[test]
    fn screen_reader_click_selects_the_targeted_row() {
        let seen = Rc::new(StdCell::new(usize::MAX));
        let s2 = Rc::clone(&seen);
        let mut r = rg().on_change(move |i, _| s2.set(i));
        let mut ctx = EventContext::new();

        let res = r.accessibility_action(AccessAction::Click, Some(2), layout(), &mut ctx);
        assert_eq!(res, EventResult::Consumed);
        assert_eq!(r.selected_index(), 2, "the AT's target row is selected");
        assert_eq!(seen.get(), 2, "handler fired as for a mouse click");
    }

    #[test]
    fn screen_reader_click_needs_a_real_row() {
        let mut r = rg().selected(1);
        let mut ctx = EventContext::new();

        // The group node itself is not activatable, and a stale option index
        // (from a snapshot of a longer list) selects nothing.
        for option in [None, Some(9)] {
            assert_eq!(
                r.accessibility_action(AccessAction::Click, option, layout(), &mut ctx),
                EventResult::Ignored,
            );
        }
        assert_eq!(r.selected_index(), 1, "selection untouched");
    }

    #[test]
    fn disabled_group_refuses_the_screen_reader_click() {
        let mut r = rg().selected(1).disabled(true);
        let mut ctx = EventContext::new();

        let res = r.accessibility_action(AccessAction::Click, Some(2), layout(), &mut ctx);
        assert_eq!(res, EventResult::Ignored);
        assert_eq!(r.selected_index(), 1, "state unchanged");
    }
}
