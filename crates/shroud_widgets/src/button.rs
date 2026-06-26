//! Button widget — clickable container with text label.

use std::time::Duration;

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Lerp, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_reactive::{Animated, Easing, Reactive};
use shroud_text::{TextAttrs, TextFamily};

/// Default hover color-transition duration — a short fade (120 ms) so the
/// button eases between its normal and hover backgrounds instead of
/// snapping. The pressed state is intentionally excluded (a press should
/// read as instant). Override with [`Button::hover_transition`].
const DEFAULT_HOVER_TRANSITION: Duration = Duration::from_millis(120);

/// A clickable button with a text label.
///
/// Has visual states for normal, hover, and pressed.
///
/// Label and all colors are stored as [`Reactive<T>`] so each accepts either
/// a literal or a signal-backed source. Dynamic variants are re-read on every
/// paint — see [`Reactive`]'s pull-based model.
/// Handler type for `Button::on_click` — takes the dispatch context so
/// handlers can queue tree mutations (`ctx.remove`, `ctx.replace_screen`).
type ClickHandler = Box<dyn FnMut(&mut EventContext)>;

pub struct Button {
    label: Reactive<String>,
    font_size: Option<f32>,
    /// Font family / weight / style the label shapes with. Defaults to the
    /// same sans-serif as plain text, so existing buttons are unaffected; the
    /// one common override is `.family(Named(..))` to draw a glyph from a
    /// bundled icon font (the label is then a single icon codepoint).
    attrs: TextAttrs,
    on_click: Option<ClickHandler>,
    // Visual state
    /// Progress of the normal→hover background fade, lazily created on the
    /// first MouseEnter/Leave (`None` until then = resting). The pressed
    /// state overrides this for instant press feedback.
    hover_anim: Option<Animated<f32>>,
    /// How long the hover fade takes; `Duration::ZERO` makes it instant.
    hover_transition: Duration,
    pressed: bool,
    focused: bool,
    // Colors (None = read from theme)
    normal_bg: Option<Reactive<Color>>,
    hover_bg: Option<Reactive<Color>>,
    press_bg: Option<Reactive<Color>>,
    text_color: Option<Reactive<Color>>,
    focus_ring_color: Option<Reactive<Color>>,
    radius: f32,
    visible: Reactive<bool>,
    /// flex-grow factor — `0.0` (default) means the button sizes to its
    /// intrinsic label width; positive values make it claim its share of
    /// leftover space along the parent's main axis. See [`Button::grow`].
    flex_grow: f32,
}

impl Button {
    /// Create a new button with the given label.
    ///
    /// Accepts anything convertible into a `String`; callers with a reactive
    /// label should use [`Button::reactive_label`] or assign via the blanket
    /// `Into<Reactive<String>>` conversions.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: Reactive::Static(label.into()),
            font_size: None,
            attrs: TextAttrs::default(),
            on_click: None,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            pressed: false,
            focused: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
            focus_ring_color: None,
            radius: 0.0,
            visible: Reactive::Static(true),
            flex_grow: 0.0,
        }
    }

    /// Create a button whose label is produced by a closure on every paint.
    ///
    /// Parallel to [`TextWidget::reactive`](crate::TextWidget::reactive) —
    /// kept as a dedicated constructor because it is the common way to drive
    /// a label from multiple signals.
    pub fn reactive_label(f: impl Fn() -> String + 'static) -> Self {
        Self {
            label: Reactive::derive(f),
            font_size: None,
            attrs: TextAttrs::default(),
            on_click: None,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            pressed: false,
            focused: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
            focus_ring_color: None,
            radius: 0.0,
            visible: Reactive::Static(true),
            flex_grow: 0.0,
        }
    }

    /// Set the click handler.
    ///
    /// The closure receives the current [`EventContext`], which is the
    /// hook for tree mutations like `ctx.remove(idx)` or
    /// `ctx.replace_screen(|tree| { ... })`. Handlers that don't need
    /// to touch the tree can ignore the parameter with `|_ctx| { ... }`.
    pub fn on_click(mut self, f: impl FnMut(&mut EventContext) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Set font size.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size = Some(px);
        self
    }

    /// Set the font family the label shapes with. Defaults to sans-serif.
    ///
    /// The primary use is an **icon button**: pass `TextFamily::Named(..)` for a
    /// font registered via `App::font` and make the label a single icon
    /// codepoint, so the glyph renders through the same shaping / tint path as
    /// any label (and recolors with `text_color`).
    pub fn family(mut self, family: TextFamily) -> Self {
        self.attrs.family = family;
        self
    }

    /// Set background color for normal state.
    ///
    /// Accepts a literal `Color`, `Signal<Color>`, `Memo<Color>`, or
    /// `Reactive::derive(...)`.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.normal_bg = Some(color.into());
        self
    }

    /// Set hover background color.
    pub fn hover_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.hover_bg = Some(color.into());
        self
    }

    /// Set how long the normal↔hover background fade takes. Defaults to a
    /// short fade (120 ms); pass [`Duration::ZERO`] for an instant flip (the
    /// pre-animation behavior). Does not affect the pressed state, which is
    /// always instant.
    pub fn hover_transition(mut self, duration: Duration) -> Self {
        self.hover_transition = duration;
        self
    }

    /// Retarget the hover fade, lazily creating the animator on first use.
    /// `to` is `1.0` for hovered, `0.0` for resting.
    fn drive_hover(&mut self, to: f32) {
        self.hover_anim
            .get_or_insert_with(|| Animated::new(0.0, self.hover_transition, Easing::EaseInOut))
            .set(to);
    }

    /// Set pressed background color.
    pub fn press_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.press_bg = Some(color.into());
        self
    }

    /// Set text color.
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    /// Override the keyboard-focus ring color. `None` (the default) reads
    /// `theme.focus.ring_color` each frame. Reactive to match the rest of
    /// `Button`'s color setters.
    pub fn focus_ring_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.focus_ring_color = Some(color.into());
        self
    }

    /// Round the button's corners by `px`. Applies to all visual states
    /// (normal / hover / press) — they share one rect. The focus ring
    /// stays rectangular for predictability; rounding the ring as well
    /// would require either a stroked SDF or four arc segments per state.
    /// Negative values are clamped to `0.0`.
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
        self
    }

    /// Flex-grow factor along the parent container's main axis.
    ///
    /// `0.0` (the default) leaves the button at its intrinsic label width.
    /// `1.0` (the common case) makes it claim the remaining space after
    /// non-grow siblings have taken theirs — the natural shape for a
    /// "row title that fills until the trailing trash icon" sidebar
    /// pattern. Negative values are clamped to `0.0`.
    pub fn grow(mut self, factor: f32) -> Self {
        self.flex_grow = factor.max(0.0);
        self
    }

    /// Toggle visibility. `false` gives `display: none` semantics — the
    /// button is removed from the layout flow, not painted, and does not
    /// receive events.
    ///
    /// Accepts a literal `bool`, `Signal<bool>`, `Memo<bool>`, or
    /// `Reactive::derive(...)`. The reactive source is re-read every frame.
    pub fn visible(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.visible = v.into();
        self
    }

    /// Whether this button currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }
}

impl Widget for Button {
    fn focusable(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        // Measured-leaf invariant: a widget that reports its size through
        // `measure` must NOT also declare a `min_size` here. When a measured
        // leaf carries a `min_size` and that min diverges from the measured
        // size, Taffy over-counts the content height of an ancestor that hugs
        // its content — a vertically-centered card (a non-root flex item with
        // no explicit height) then resolves taller than its laid-out children
        // and leaves dead space below the last child (the knot lock-screen
        // gap). Padding amplifies the over-count but isn't required to trigger
        // it. So the button's minimum height (font + the 16px Taffy adds for
        // padding) lives in `measure` via `height.max(font_size)`, giving the
        // same visual height without a style `min_size`. See `TextWidget::style`
        // for the same fix; `SecureInput` has no `measure`, so it's immune and
        // keeps its `min_height`.
        let mut style = FlexStyle::new().padding(8.0).center();
        if self.flex_grow > 0.0 {
            style = style.grow(self.flex_grow);
        }
        style
    }

    fn visible(&self) -> bool {
        self.visible.get()
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);
        let label = self.label.get();
        if label.is_empty() {
            return Some(Size::new(0.0, font_size));
        }
        let line_height = font_size * 1.2;
        // Compute max-content first so narrow `available_width` probes from
        // Taffy's flex algorithm don't collapse the label to its min-content
        // shape. Only wrap when the natural width actually exceeds the
        // available width (e.g., long label in a narrow container).
        let natural =
            ctx.text_engine
                .shape_text_attrs(&label, font_size, line_height, None, &self.attrs);
        let shaped = if let Some(aw) = available_width {
            if natural.width > aw {
                ctx.text_engine.shape_text_attrs(
                    &label,
                    font_size,
                    line_height,
                    Some(aw),
                    &self.attrs,
                )
            } else {
                natural
            }
        } else {
            natural
        };
        // Height must account for at least the font so single-line buttons
        // have the same visual height regardless of font ascent/descent.
        // Ceil width so Taffy's pixel rounding never shortens us below the
        // natural shape width (see TextWidget::measure for the full story).
        let height = shaped.height.max(font_size).ceil();
        Some(Size::new(shaped.width.ceil(), height))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let colors = &ctx.theme.colors;
        let normal = self
            .normal_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary);
        let hover = self
            .hover_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary_hover);
        let press = self
            .press_bg
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.primary_pressed);
        let text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.on_primary);
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);

        // Pressed reads as instant; otherwise fade between normal and hover.
        // `get()` votes for another frame while the fade is in flight, and
        // the endpoints short-circuit so a settled state paints its exact
        // color (float lerp isn't bit-exact at t==1).
        let bg = if self.pressed {
            press
        } else {
            let t = self.hover_anim.as_ref().map_or(0.0, |a| a.get());
            if t >= 1.0 {
                hover
            } else if t <= 0.0 {
                normal
            } else {
                normal.lerp(&hover, t)
            }
        };

        // Background
        ctx.fill_rect_rounded(layout, bg, self.radius);

        // Label text (centered within the button)
        let label = self.label.get();
        if !label.is_empty() {
            let shaped = ctx.text_engine.shape_text_attrs(
                &label,
                font_size,
                font_size * 1.2,
                Some(layout.size.width),
                &self.attrs,
            );

            // Center the text block within the button
            let text_x = layout.origin.x + (layout.size.width - shaped.width) / 2.0;
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

        if self.focused && ctx.focus_visible() {
            let override_color = self.focus_ring_color.as_ref().map(|c| c.get());
            ctx.paint_focus_ring(layout, override_color, self.radius);
        }
    }

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.drive_hover(1.0);
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.drive_hover(0.0);
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
            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }
            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }
            // Keyboard activation: Enter triggers click while focused.
            // Browser parity — both Enter and Space activate a button, but
            // Space arrives as `CharInput { ch: ' ' }` (winit routes it
            // through the character pipeline alongside other printable keys).
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            } if self.focused => {
                if let Some(handler) = &mut self.on_click {
                    handler(ctx);
                }
                EventResult::Consumed
            }
            WidgetEvent::CharInput { ch: ' ' } if self.focused => {
                if let Some(handler) = &mut self.on_click {
                    handler(ctx);
                }
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}
