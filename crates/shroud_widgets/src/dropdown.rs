//! Dropdown widget — popover-based single-select.
//!
//! A [`Dropdown`] renders as a button-like trigger that shows the
//! currently-selected option label. Clicking the trigger opens a popover
//! layer with the option list, anchored under the trigger (flipping above
//! when below would overflow the viewport, per [`Placement::Auto`]).
//!
//! The popover is dismissed on outside-click, Escape, or option click.
//! Selection writes back to the bound `Signal<usize>`, which apps observe
//! via paint-time reads or a [`Memo`](shroud_reactive::Memo).
//!
//! Keyboard nav inside the popover (arrow keys, type-ahead) is deferred to
//! Phase 22.5 — for now, Enter/Space on the focused trigger toggles the
//! popover and selection is mouse-only.

use std::cell::Cell;
use std::rc::Rc;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::layer::{LayerAnchor, LayerOptions, Placement};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::{Reactive, Signal};

/// Single-select dropdown.
///
/// The trigger displays the option at the bound signal's current index.
/// When the index is out of range, the placeholder (if set) is shown.
///
/// # Example (conceptual)
///
/// ```ignore
/// let selected = Signal::new(0_usize);
/// let dropdown = Dropdown::new(
///     vec!["Light".into(), "Dark".into(), "System".into()],
///     selected,
/// )
/// .placeholder("Theme");
/// ```
pub struct Dropdown {
    options: Vec<String>,
    selected: Signal<usize>,
    placeholder: Option<String>,

    hovered: bool,
    pressed: bool,
    focused: bool,

    /// Shared with the popover root via `Rc`. Set to `true` while the layer
    /// is on the stack; flipped back to `false` by [`DropdownPopover`]'s
    /// `Drop` impl, which fires regardless of how the layer was dismissed
    /// (outside-click, Escape, or option selection). Keeps the trigger's
    /// open/closed state in sync without threading close callbacks through
    /// every dismiss path.
    open: Rc<Cell<bool>>,

    // Trigger styling — `None` reads from the theme each paint.
    radius: f32,
    background: Option<Reactive<Color>>,
    text_color: Option<Reactive<Color>>,
    border_color: Option<Reactive<Color>>,
    focus_ring_color: Option<Reactive<Color>>,
    font_size: Option<f32>,
    visible: Reactive<bool>,
}

impl Dropdown {
    /// Create a dropdown with the given option labels bound to a selection
    /// signal. The signal seeds the initial display and is rewritten on
    /// each option click.
    pub fn new(options: Vec<String>, selected: Signal<usize>) -> Self {
        Self {
            options,
            selected,
            placeholder: None,
            hovered: false,
            pressed: false,
            focused: false,
            open: Rc::new(Cell::new(false)),
            radius: 4.0,
            background: None,
            text_color: None,
            border_color: None,
            focus_ring_color: None,
            font_size: None,
            visible: Reactive::Static(true),
        }
    }

    /// Set placeholder text shown when the bound signal points at an
    /// out-of-range index.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
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

    /// Round the trigger's corners. Negative values clamp to `0.0`.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
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
        self.focused
    }

    fn current_label(&self) -> String {
        let idx = self.selected.get();
        self.options
            .get(idx)
            .cloned()
            .unwrap_or_else(|| self.placeholder.clone().unwrap_or_default())
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
        let options = self.options.clone();
        let selected = self.selected;
        let open_guard = Rc::clone(&self.open);

        let layer_options = LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
            rect: trigger_rect,
            prefer: Placement::Auto,
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
                    OptionItem::new(label.clone(), move |inner_ctx| {
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
        let font_size = self.font_size.unwrap_or(16.0);
        FlexStyle::new()
            .padding_trbl(8.0, 12.0, 8.0, 12.0)
            .min_height(font_size + 16.0)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let line_height = font_size * 1.2;
        // Size to the widest option (plus chevron + padding gutter) so the
        // trigger does not jump as the selection changes. Placeholder is
        // measured too so a never-selected trigger has room for its text.
        let max_label_width = self
            .options
            .iter()
            .chain(self.placeholder.iter())
            .map(|label| {
                ctx.text_engine
                    .shape_text(label, font_size, line_height, None)
                    .width
            })
            .fold(0.0_f32, f32::max);
        // 24 = 12px left padding + 12px right gutter for the chevron; the
        // chevron itself is ~font_size wide.
        let needed_width = max_label_width + font_size + 8.0;
        let width = match available_width {
            Some(aw) if needed_width > aw => aw,
            _ => needed_width,
        };
        let height = (font_size + 16.0).ceil();
        Some(Size::new(width.ceil(), height))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let colors = &ctx.theme.colors;
        let bg = self
            .background
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.input_background);
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.on_surface);
        let border = self
            .border_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.input_border);
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);

        ctx.fill_rect_rounded(layout, bg, self.radius);

        // 1px border on each side. Same shape as `Input::paint` — sharp
        // corners are fine; the underlying fill carries the radius and the
        // hairline edges over rounded corners read as a subtle stroke.
        let b = 1.0;
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.bottom() - b, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, b, layout.size.height),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.right() - b, layout.origin.y, b, layout.size.height),
            border,
        );

        // Left-aligned label.
        let label = self.current_label();
        let chevron = "\u{25BE}"; // ▾
        let chevron_shaped = ctx
            .text_engine
            .shape_text(chevron, font_size, font_size * 1.2, None);
        let chevron_w = chevron_shaped.width;
        let right_gutter = chevron_w + 16.0;
        if !label.is_empty() {
            let max_label_w = (layout.size.width - 12.0 - right_gutter).max(0.0);
            let shaped =
                ctx.text_engine
                    .shape_text(&label, font_size, font_size * 1.2, Some(max_label_w));
            let text_x = layout.origin.x + 12.0;
            let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
            for glyph in &shaped.glyphs {
                if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                    ctx.draw_glyph(
                        text_x as i32 + glyph.x,
                        text_y as i32 + glyph.y,
                        image,
                        text_color,
                        glyph.cache_key,
                    );
                }
            }
        }

        // Chevron on the right. Color matches the label; muted variants can
        // be wired in once Theme has a chevron token.
        let chev_x = layout.origin.x + layout.size.width - 12.0 - chevron_w;
        let chev_y = layout.origin.y + (layout.size.height - chevron_shaped.height) / 2.0;
        for glyph in &chevron_shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    chev_x as i32 + glyph.x,
                    chev_y as i32 + glyph.y,
                    image,
                    text_color,
                    glyph.cache_key,
                );
            }
        }

        if self.focused {
            let override_color = self.focus_ring_color.as_ref().map(|c| c.get());
            ctx.paint_focus_ring(layout, override_color);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.hovered = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.hovered = false;
                self.pressed = false;
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.pressed = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.pressed {
                    self.pressed = false;
                    self.toggle(layout, ctx);
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }
            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            } if self.focused => {
                self.toggle(layout, ctx);
                EventResult::Consumed
            }
            // Space arrives as a character (matches Button activation path).
            WidgetEvent::CharInput { ch: ' ' } if self.focused => {
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
    radius: f32,
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
        ctx.fill_rect_rounded(layout, bg, self.radius);
        // Subtle 1px border so the popover separates from the surface
        // it overlaps.
        let border = ctx.theme.colors.input_border;
        let b = 1.0;
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.bottom() - b, layout.size.width, b),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.origin.x, layout.origin.y, b, layout.size.height),
            border,
        );
        ctx.fill_rect(
            Rect::new(layout.right() - b, layout.origin.y, b, layout.size.height),
            border,
        );
    }
}

impl Drop for DropdownPopover {
    fn drop(&mut self) {
        self.open.set(false);
    }
}

/// Click handler for [`OptionItem`]. Same shape as `Button`'s
/// `ClickHandler`, kept as a type alias so the struct field stays inside
/// `clippy::type_complexity`.
type OptionClickHandler = Box<dyn FnMut(&mut EventContext)>;

/// Internal: a single row in the dropdown popover. Button-like, left-aligned,
/// theme-driven hover highlight. Not exported — apps that want a
/// general-purpose menu item can build one on top of `Button` /
/// `Container`.
struct OptionItem {
    label: String,
    on_click: Option<OptionClickHandler>,
    hovered: bool,
    pressed: bool,
}

impl OptionItem {
    fn new(label: String, on_click: impl FnMut(&mut EventContext) + 'static) -> Self {
        Self {
            label,
            on_click: Some(Box::new(on_click)),
            hovered: false,
            pressed: false,
        }
    }
}

impl Widget for OptionItem {
    fn style(&self) -> FlexStyle {
        FlexStyle::new()
            .padding_trbl(6.0, 12.0, 6.0, 12.0)
            .min_height(28.0)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = ctx.theme.typography.body.font_size;
        if self.label.is_empty() {
            return Some(Size::new(0.0, font_size));
        }
        let line_height = font_size * 1.2;
        let natural = ctx
            .text_engine
            .shape_text(&self.label, font_size, line_height, None);
        let shaped = match available_width {
            Some(aw) if natural.width > aw => {
                ctx.text_engine
                    .shape_text(&self.label, font_size, line_height, Some(aw))
            }
            _ => natural,
        };
        Some(Size::new(
            shaped.width.ceil(),
            shaped.height.max(font_size).ceil(),
        ))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        // Copy out the theme tokens we need so the immutable borrow drops
        // before the mutable PaintContext calls below.
        let surface_variant = ctx.theme.colors.surface_variant;
        let text_color = ctx.theme.colors.on_surface;
        let font_size = ctx.theme.typography.body.font_size;

        // Hover highlight uses surface_variant — already in the palette as
        // the "subtle container variant" tone, which reads naturally as a
        // row hover against the popover's surface background.
        let bg = if self.hovered {
            surface_variant
        } else {
            Color::TRANSPARENT
        };
        if bg.a > 0.0 {
            ctx.fill_rect(layout, bg);
        }

        let line_height = font_size * 1.2;
        let max_w = layout.size.width.max(0.0);
        let shaped = ctx
            .text_engine
            .shape_text(&self.label, font_size, line_height, Some(max_w));
        let text_x = layout.origin.x;
        let text_y = layout.origin.y + (layout.size.height - shaped.height) / 2.0;
        for glyph in &shaped.glyphs {
            if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                ctx.draw_glyph(
                    text_x as i32 + glyph.x,
                    text_y as i32 + glyph.y,
                    image,
                    text_color,
                    glyph.cache_key,
                );
            }
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.hovered = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.hovered = false;
                self.pressed = false;
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.pressed = true;
                EventResult::Consumed
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.pressed {
                    self.pressed = false;
                    if let Some(handler) = &mut self.on_click {
                        handler(ctx);
                    }
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
