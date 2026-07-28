//! Dropdown widget — popover-based single-select.
//!
//! A [`Dropdown`] renders as a button-like trigger that shows the
//! currently-selected option label. Clicking the trigger opens a popover
//! layer with the option list, anchored under the trigger (flipping above
//! when below would overflow the viewport, per [`Placement::Auto`]).
//!
//! The popover is dismissed on outside-click, Escape, or option click.
//! Selection writes back to the bound `Signal<usize>`, which apps observe
//! via paint-time reads or a [`Memo`](sindon_reactive::Memo).
//!
//! Enter / Space on the focused trigger toggles the popover. Selection from
//! the keyboard is then the popover's own: its rows are
//! [`MenuItem`]s, which are tab stops, so Tab / ↓ steps into the list and
//! Enter chooses — see that widget's docs. Escape hands focus back to the
//! trigger. Type-ahead (jump to the option whose label starts with the typed
//! letter) is not implemented.

use std::cell::Cell;
use std::rc::Rc;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::interaction::{InteractionState, Release};
use crate::layer::{HAlign, LayerAnchor, LayerOptions, Placement};
use crate::menu_item::MenuItem;
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{Color, FocusIndicator, Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::{Reactive, Signal};

/// Single-select dropdown.
///
/// The trigger displays the option at the bound signal's current index.
/// When the index is out of range, the placeholder (if set) is shown.
///
/// Both label sources have a reactive form —
/// [`reactive_options`](Self::reactive_options) and
/// [`reactive_placeholder`](Self::reactive_placeholder) — for apps whose
/// labels change in place (a language switch) rather than only when the
/// screen is rebuilt.
///
/// # Example (conceptual)
///
/// ```
/// # use sindon_reactive::Signal;
/// # use sindon_widgets::Dropdown;
/// let selected = Signal::new(0_usize);
/// let dropdown = Dropdown::new(
///     vec!["Light".into(), "Dark".into(), "System".into()],
///     selected,
/// )
/// .placeholder("Theme");
/// ```
pub struct Dropdown {
    // Option labels. Reactive so a language switch (or any other signal-driven
    // relabel) reaches the trigger and the next-opened popover on the following
    // frame, rather than waiting for the screen to be rebuilt — the same
    // reasoning as `Input`'s reactive placeholder. Read through `with` in the
    // per-frame paths: `get()` would deep-clone the whole list every measure.
    options: Reactive<Vec<String>>,
    selected: Signal<usize>,
    // Label shown when the bound index is out of range. Empty means unset.
    placeholder: Reactive<String>,

    /// Pointer / keyboard interaction flags — see [`InteractionState`].
    state: InteractionState,

    /// Shared with the popover root via `Rc`. Set to `true` while the layer
    /// is on the stack; flipped back to `false` by [`DropdownPopover`]'s
    /// `Drop` impl, which fires regardless of how the layer was dismissed
    /// (outside-click, Escape, or option selection). Keeps the trigger's
    /// open/closed state in sync without threading close callbacks through
    /// every dismiss path.
    open: Rc<Cell<bool>>,

    // Trigger styling — `None` reads from the theme each paint.
    radius: Option<f32>,
    background: Option<Reactive<Color>>,
    text_color: Option<Reactive<Color>>,
    border_color: Option<Reactive<Color>>,
    focus_ring_color: Option<Reactive<Color>>,
    font_size: Option<f32>,
    // Trigger box metrics. `padding_*` are the inner insets on each axis
    // (Tailwind `px-*`/`py-*`); `min_height` is an explicit border-box floor
    // that overrides the one-line default. See `measure`.
    padding_x: f32,
    padding_y: f32,
    min_height: Option<f32>,
    visible: Reactive<bool>,
}

impl Dropdown {
    /// Create a dropdown with the given option labels bound to a selection
    /// signal. The signal seeds the initial display and is rewritten on
    /// each option click.
    pub fn new(options: Vec<String>, selected: Signal<usize>) -> Self {
        Self {
            options: Reactive::Static(options),
            selected,
            placeholder: Reactive::Static(String::new()),
            state: InteractionState::default(),
            open: Rc::new(Cell::new(false)),
            radius: None,
            background: None,
            text_color: None,
            border_color: None,
            focus_ring_color: None,
            font_size: None,
            padding_x: 12.0,
            padding_y: 8.0,
            min_height: None,
            visible: Reactive::Static(true),
        }
    }

    /// Set placeholder text shown when the bound signal points at an
    /// out-of-range index.
    ///
    /// Callers whose prompt can change while the dropdown is on screen — a
    /// language switch, most commonly — should use
    /// [`reactive_placeholder`](Self::reactive_placeholder) instead.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Reactive::Static(text.into());
        self
    }

    /// Set a placeholder produced by a closure on every frame.
    ///
    /// Parallel to [`Input::reactive_placeholder`](crate::Input::reactive_placeholder):
    /// the closure is re-read each frame, so a signal write (`language.set(Ja)`)
    /// reaches the trigger on the next one — no tree rebuild required.
    pub fn reactive_placeholder(mut self, f: impl Fn() -> String + 'static) -> Self {
        self.placeholder = Reactive::derive(f);
        self
    }

    /// Set option labels produced by a closure on every frame.
    ///
    /// The reactive counterpart to the list passed to [`new`](Self::new), for
    /// the same relabel-without-rebuild reason as
    /// [`reactive_placeholder`](Self::reactive_placeholder). The bound
    /// `Signal<usize>` indexes into whatever the closure last returned, so a
    /// relabel that keeps the option *order* (a translation) keeps the
    /// selection; a closure that reorders or resizes the list should write the
    /// signal to match.
    ///
    /// The trigger re-measures against the new labels on the next frame (it
    /// sizes to the widest option). An already-open popover keeps the labels it
    /// was built with until it is dismissed and re-opened — a relabel mid-open
    /// would mean rebuilding the layer's children under the cursor.
    pub fn reactive_options(mut self, f: impl Fn() -> Vec<String> + 'static) -> Self {
        self.options = Reactive::derive(f);
        self
    }

    /// Set the font size for the trigger label.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set the trigger background color. Defaults to `theme.input_background`.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.background = Some(color.into());
        self
    }

    /// Set the trigger label color. Defaults to `theme.on_surface`.
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    /// Set the trigger border color. Defaults to `theme.input_border`.
    pub fn border_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.border_color = Some(color.into());
        self
    }

    /// Override the keyboard-focus ring color.
    pub fn focus_ring_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.focus_ring_color = Some(color.into());
        self
    }

    /// Round the trigger's corners (and the popover, which matches). Unset,
    /// both round to `theme.shape.radius_sm`; pass `0.0` for square corners.
    /// Negative values clamp to `0.0`.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = Some(px.max(0.0));
        self
    }

    /// Horizontal inset (Tailwind `px-*`) between the trigger edge and its
    /// label / chevron. Defaults to `12.0`. Negative values clamp to `0.0`.
    pub fn padding_x(mut self, px: f32) -> Self {
        self.padding_x = px.max(0.0);
        self
    }

    /// Vertical inset (Tailwind `py-*`) added above and below the label.
    /// Defaults to `8.0`; drives the trigger's border-box height together with
    /// the line height. Negative values clamp to `0.0`.
    pub fn padding_y(mut self, px: f32) -> Self {
        self.padding_y = px.max(0.0);
        self
    }

    /// Explicit border-box height floor for the trigger, overriding the
    /// one-line default (`line_height + 2 * padding_y`). Negative values clamp
    /// to `0.0`.
    pub fn min_height(mut self, px: f32) -> Self {
        self.min_height = Some(px.max(0.0));
        self
    }

    /// Toggle visibility. `false` gives `display: none` semantics.
    pub fn visible(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.visible = v.into();
        self
    }

    /// Whether the popover is currently open.
    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Whether the trigger currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    fn current_label(&self) -> String {
        let idx = self.selected.get();
        self.options
            .with(|opts| opts.get(idx).cloned())
            .unwrap_or_else(|| self.placeholder.get())
    }

    fn toggle(&self, layout: Rect, ctx: &mut EventContext) {
        if self.open.get() {
            ctx.pop_top_layer();
            // open flips to false via DropdownPopover::drop after drain.
        } else {
            self.open_popover(layout, ctx);
        }
    }

    fn open_popover(&self, trigger_rect: Rect, ctx: &mut EventContext) {
        // Snapshot the labels for the layer's children. A reactive relabel
        // while the popover is up is not reflected until it re-opens (see
        // `reactive_options`).
        let options = self.options.get();
        let selected = self.selected;
        let open_guard = Rc::clone(&self.open);

        let layer_options = LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
            rect: trigger_rect,
            prefer: Placement::Auto,
            // Left-aligned under the trigger, matching a native <select>.
            align: HAlign::Start,
        });

        let popover = DropdownPopover {
            background: Color::TRANSPARENT, // resolved from theme at paint time
            radius: self.radius,
            open: Rc::clone(&open_guard),
            min_width: trigger_rect.size.width,
        };
        ctx.push_layer(layer_options, popover, move |tree, popover_root| {
            for (idx, label) in options.iter().enumerate() {
                let signal = selected;
                tree.add_child(
                    popover_root,
                    MenuItem::new(label.clone(), move |inner_ctx| {
                        signal.set(idx);
                        inner_ctx.pop_top_layer();
                    }),
                );
            }
        });
        // The push command drains after dispatch; mark `open` synchronously
        // so a same-frame second click takes the toggle-closed branch.
        open_guard.set(true);
    }
}

impl Widget for Dropdown {
    fn focusable(&self) -> bool {
        true
    }

    fn visible(&self) -> bool {
        self.visible.get()
    }

    fn style(&self) -> FlexStyle {
        // Measured-leaf invariant (see `Button::style`): a widget that reports
        // its size through `measure` must NOT also declare a `min_size` here.
        // When the two diverge, Taffy over-counts the content height of a
        // content-hugging ancestor (a vertically-centered card resolves taller
        // than its laid-out children and leaves dead space below the last one).
        // The trigger's minimum height lives in `measure` instead — see the
        // height floor there.
        FlexStyle::new().padding_trbl(
            self.padding_y,
            self.padding_x,
            self.padding_y,
            self.padding_x,
        )
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = font_size * 1.2;
        // Size to the widest option (plus chevron + padding gutter) so the
        // trigger does not jump as the selection changes. Placeholder is
        // measured too so a never-selected trigger has room for its text.
        // Reactive labels are read once per measure and walked by reference —
        // `get()` on the option list would deep-clone every label here, and
        // Taffy calls `measure` several times a frame.
        let mut max_label_width = self.options.with(|opts| {
            opts.iter()
                .map(|label| {
                    ctx.text_engine
                        .measure_text(label, font_size, line_height, None)
                        .0
                })
                .fold(0.0_f32, f32::max)
        });
        let placeholder = self.placeholder.get();
        if !placeholder.is_empty() {
            let w = ctx
                .text_engine
                .measure_text(&placeholder, font_size, line_height, None)
                .0;
            max_label_width = max_label_width.max(w);
        }
        // Content width = widest label + gap + chevron (~font_size wide). Taffy
        // adds the horizontal padding (`2 * padding_x`) on top to form the box.
        let needed_width = max_label_width + font_size + 8.0;
        let width = match available_width {
            Some(aw) if needed_width > aw => aw,
            _ => needed_width,
        };
        // Trigger height. A measured leaf must not carry a style `min_size`
        // (Taffy over-counts a content-hugging ancestor otherwise — the
        // centered-card dead-space bug; see `Dropdown::style`), so the height
        // floor lives here. One text line is the natural content height; Taffy
        // adds the vertical padding (`2 * padding_y`) on top to form the border
        // box. An explicit `min_height` is a border-box floor, so subtract the
        // padding to compare it against this content height.
        let content_floor = self
            .min_height
            .map(|h| (h - 2.0 * self.padding_y).max(0.0))
            .unwrap_or(0.0);
        let height = line_height.max(content_floor).ceil();
        Some(Size::new(width.ceil(), height))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let colors = &ctx.theme.colors;
        let hover_bg = ctx.theme.hover.bg;
        let base_bg = self
            .background
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.input_background);
        // Hover overrides the resting bg; the popover stays open while
        // hovered, so the trigger keeps the highlight as long as the
        // cursor sits over it — matches OS-native combobox feel.
        let bg = if self.state.hovered {
            hover_bg
        } else {
            base_bg
        };
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.on_surface);
        // Focus indicator: the trigger always draws a border, so `Border`
        // mode recolors it to the focus color and suppresses the ring below.
        let focus_active = self.state.focused && ctx.focus_visible();
        let border_focus = focus_active && ctx.theme.focus.indicator == FocusIndicator::Border;
        let border = if border_focus {
            self.focus_ring_color
                .as_ref()
                .map(|c| c.get())
                .unwrap_or(ctx.theme.focus.ring_color)
        } else {
            self.border_color
                .as_ref()
                .map(|c| c.get())
                .unwrap_or(colors.input_border)
        };
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);

        // Unset radius rounds to the theme's small-control radius; `.radius(px)`
        // overrides (0.0 = square).
        let radius = self.radius.unwrap_or(ctx.theme.shape.radius_sm);
        ctx.fill_rect_rounded(layout, bg, radius);

        // 1px border following the same rounded corners as the fill — one SDF
        // stroke, matching `Input`/`SecureInput` (rather than four sharp
        // hairlines that square off the corners).
        ctx.stroke_rect_rounded(layout, border, radius, 1.0);

        // Left-aligned label, inset by the horizontal padding; the chevron sits
        // padding-inset on the right, with an 8px gutter before the label.
        let label = self.current_label();
        let chevron = "\u{25BE}"; // ▾
        let chevron_shaped = ctx
            .text_engine
            .shape_text(chevron, font_size, font_size * 1.2, None);
        let chevron_w = chevron_shaped.width;
        let text_x = layout.origin.x + self.padding_x;
        let chev_x = layout.right() - self.padding_x - chevron_w;
        if !label.is_empty() {
            let max_label_w = (chev_x - 8.0 - text_x).max(0.0);
            let shaped =
                ctx.text_engine
                    .shape_text(&label, font_size, font_size * 1.2, Some(max_label_w));
            let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
            for glyph in &shaped.glyphs {
                if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                    ctx.draw_glyph(
                        text_x + glyph.x,
                        text_y + glyph.y,
                        image,
                        text_color,
                        glyph.cache_key,
                    );
                }
            }
        }

        // Chevron on the right. Color matches the label; muted variants can
        // be wired in once Theme has a chevron token.
        let chev_y = layout.origin.y + (layout.size.height - chevron_shaped.height) / 2.0;
        for glyph in &chevron_shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    chev_x + glyph.x,
                    chev_y + glyph.y,
                    image,
                    text_color,
                    glyph.cache_key,
                );
            }
        }

        // Ring in Ring mode; suppressed when Border mode recolored the border.
        if focus_active && !border_focus {
            let override_color = self.focus_ring_color.as_ref().map(|c| c.get());
            ctx.paint_focus_ring(layout, override_color, radius);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        // The trigger has no disabled state, so it never latches inertly —
        // pass `false` throughout. Flag bookkeeping and the clear-vs-latch
        // discipline live in [`InteractionState`].
        match event {
            WidgetEvent::MouseEnter => {
                self.state.enter(false);
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.state.leave();
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.state.press(false);
                EventResult::Consumed
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => match self.state.release(false) {
                Release::Fire => {
                    self.toggle(layout, ctx);
                    EventResult::Consumed
                }
                Release::Cancelled => EventResult::Consumed,
                Release::Idle => EventResult::Ignored,
            },
            WidgetEvent::FocusGained => {
                self.state.focus_gained(false);
                EventResult::Ignored
            }
            WidgetEvent::FocusLost => {
                self.state.focus_lost();
                EventResult::Ignored
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            } if self.state.focused => {
                self.toggle(layout, ctx);
                EventResult::Consumed
            }
            // Space arrives as a character (matches Button activation path).
            WidgetEvent::CharInput { ch: ' ' } if self.state.focused => {
                self.toggle(layout, ctx);
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Internal popover root for the option list. Carries the `open` guard so
/// any dismiss path (outside-click, Escape, option click, programmatic
/// pop) flips the dropdown's `is_open` flag back to false via `Drop`.
struct DropdownPopover {
    /// Resolved at paint time from theme.surface if `Color::TRANSPARENT`.
    background: Color,
    /// Mirrors the trigger's radius. `None` reads `theme.shape.radius_sm`.
    radius: Option<f32>,
    open: Rc<Cell<bool>>,
    /// Minimum width — matches the trigger so the popover is at least as
    /// wide. Wider option labels still grow the popover.
    min_width: f32,
}

impl Widget for DropdownPopover {
    fn style(&self) -> FlexStyle {
        FlexStyle::new()
            .column()
            .padding(4.0)
            .min_width(self.min_width)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let bg = if self.background == Color::TRANSPARENT {
            ctx.theme.colors.surface
        } else {
            self.background
        };
        let radius = self.radius.unwrap_or(ctx.theme.shape.radius_sm);
        ctx.fill_rect_rounded(layout, bg, radius);
        // Subtle 1px border so the popover separates from the surface it
        // overlaps — one rounded SDF stroke, matching the trigger.
        let border = ctx.theme.colors.input_border;
        ctx.stroke_rect_rounded(layout, border, radius, 1.0);
    }
}

impl Drop for DropdownPopover {
    fn drop(&mut self) {
        self.open.set(false);
    }
}
