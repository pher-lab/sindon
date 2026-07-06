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
/// Handler type for [`Button::on_click_rect`] — like [`ClickHandler`] but also
/// gets the button's own layout rect, for anchoring a popover to the trigger
/// itself. The rect-returning, a11y-complete counterpart to
/// [`Container::on_press_rect`](crate::Container::on_press_rect).
type ClickRectHandler = Box<dyn FnMut(Rect, &mut EventContext)>;

pub struct Button {
    label: Reactive<String>,
    font_size: Option<f32>,
    /// Font family / weight / style the label shapes with. Defaults to the
    /// same sans-serif as plain text, so existing buttons are unaffected; the
    /// one common override is `.family(Named(..))` to draw a glyph from a
    /// bundled icon font (the label is then a single icon codepoint).
    attrs: TextAttrs,
    on_click: Option<ClickHandler>,
    /// Optional activation handler that also receives the button's own layout
    /// rect (rather than nothing), for anchoring a popover to the trigger
    /// itself. Fires at the same moments as `on_click`. See
    /// [`Button::on_click_rect`].
    on_click_rect: Option<ClickRectHandler>,
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
    /// Label color at full hover, faded in on the same curve as the
    /// background. `None` keeps `text_color` constant. Lets a text-only
    /// button (a link) darken its label on hover the way `hover:text-*`
    /// does, without a background change.
    hover_text_color: Option<Reactive<Color>>,
    focus_ring_color: Option<Reactive<Color>>,
    /// Background at full disabled, replacing the normal fill when
    /// [`disabled`](Self::disabled) reads true. `None` dims the normal
    /// background (and label) to half alpha — a theme-agnostic "greyed out"
    /// that reads on any surface. Set via [`disabled_background`](Self::disabled_background).
    disabled_bg: Option<Reactive<Color>>,
    /// Whether the button is inert: it drops its hover/press feedback, skips
    /// keyboard focus, and does not fire `on_click`. Reactive so a form can
    /// gate its submit on a signal. Defaults to always-enabled.
    disabled: Reactive<bool>,
    radius: f32,
    /// Horizontal padding (px) between the button edge and its label, each
    /// side — Tailwind `px-*`. Default 8.
    pad_x: f32,
    /// Vertical padding (px), top and bottom — Tailwind `py-*`. Default 8.
    pad_y: f32,
    /// Explicit minimum box width (px). `None` sizes to the label. Lets a row
    /// of icon buttons stay uniform despite differing glyph advances. See
    /// [`Button::min_width`].
    min_width: Option<f32>,
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
            on_click_rect: None,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            pressed: false,
            focused: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
            hover_text_color: None,
            focus_ring_color: None,
            disabled_bg: None,
            disabled: Reactive::Static(false),
            radius: 0.0,
            pad_x: 8.0,
            pad_y: 8.0,
            min_width: None,
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
            on_click_rect: None,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            pressed: false,
            focused: false,
            normal_bg: None,
            hover_bg: None,
            press_bg: None,
            text_color: None,
            hover_text_color: None,
            focus_ring_color: None,
            disabled_bg: None,
            disabled: Reactive::Static(false),
            radius: 0.0,
            pad_x: 8.0,
            pad_y: 8.0,
            min_width: None,
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

    /// Set an activation handler that also receives the button's own laid-out
    /// rect — the rect-returning counterpart to [`on_click`](Self::on_click).
    ///
    /// It fires at the same three moments as `on_click` (mouse release, Enter,
    /// Space) but hands back the button's box instead of nothing: everything a
    /// click handler needs plus the geometry to anchor a popover *to the
    /// button*. Because the rect is the button's own layout box — not a cursor
    /// point — the anchor is well-defined for keyboard activation too, so a
    /// header menu opened this way is fully Tab/Enter-operable. That is the one
    /// thing [`Container::on_press_rect`](crate::Container::on_press_rect) can't
    /// give you: it also reports the trigger's rect, but fires on the raw press
    /// and leaves its `Container` trigger unfocusable, so keyboard users can't
    /// reach it. Reach for this on any menu button that must be accessible; keep
    /// `on_press_rect` for a pure pointer affordance.
    ///
    /// `push_layer` translates an `AnchorRect` built from this rect into
    /// viewport space, so the popover lands correctly even nested in a modal.
    /// Both `on_click` and `on_click_rect` fire when both are set.
    pub fn on_click_rect(mut self, f: impl FnMut(Rect, &mut EventContext) + 'static) -> Self {
        self.on_click_rect = Some(Box::new(f));
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

    /// Set the label color at full hover. The label fades from
    /// [`text_color`](Self::text_color) to this on the same curve as the
    /// background hover fade. Use for a text-only button — a link that
    /// darkens its label on hover (`hover:text-gray-700`) — typically paired
    /// with transparent `hover_background` / `press_background` so no fill
    /// appears. Leaving it unset keeps the label color constant.
    pub fn hover_text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.hover_text_color = Some(color.into());
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

    /// Horizontal padding (px) between the button edge and its label, on each
    /// side — Tailwind `px-*`. Default 8. Negative values clamp to `0.0`.
    pub fn padding_x(mut self, px: f32) -> Self {
        self.pad_x = px.max(0.0);
        self
    }

    /// Vertical padding (px), top and bottom — Tailwind `py-*`. Default 8. The
    /// natural way to grow a filled button to a design's height (`py-3` ≈ a
    /// 48px control) without a fixed `height` that would clip a wrapped label.
    /// Negative values clamp to `0.0`.
    pub fn padding_y(mut self, px: f32) -> Self {
        self.pad_y = px.max(0.0);
        self
    }

    /// Set a minimum box width (px). The button still grows past this to fit a
    /// wider label, but never shrinks below it — so a toolbar row of icon
    /// buttons stays uniform even though `B` / `I` / `1.` shape to different
    /// advances. Negative values clamp to `0.0`.
    pub fn min_width(mut self, px: f32) -> Self {
        self.min_width = Some(px.max(0.0));
        self
    }

    /// Gate the button on a disabled state (reactive).
    ///
    /// While `true` the button drops its hover / press feedback, is skipped by
    /// keyboard focus (Tab), and does not fire [`on_click`](Self::on_click) —
    /// the natural shape for a form submit that stays inert until its fields
    /// validate. The disabled fill defaults to the normal background dimmed to
    /// half alpha (a theme-agnostic "greyed out"); override it with
    /// [`disabled_background`](Self::disabled_background). Accepts a literal
    /// `bool` or a signal-backed source.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Override the background painted while [`disabled`](Self::disabled) reads
    /// true (Tailwind `disabled:bg-*`), replacing the default half-alpha dim.
    /// Reactive, like the other color setters.
    pub fn disabled_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.disabled_bg = Some(color.into());
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

    /// Fire the activation handlers. The shared path for the three activation
    /// routes — a left-mouse release, Enter, and Space — so they behave
    /// identically. `on_click` gets no argument; `on_click_rect` gets the
    /// button's own layout rect (to anchor a popover to the trigger). Both
    /// fire when both are set; a no-op when neither is.
    fn activate(&mut self, layout: Rect, ctx: &mut EventContext) {
        if let Some(handler) = &mut self.on_click {
            handler(ctx);
        }
        if let Some(handler) = &mut self.on_click_rect {
            handler(layout, ctx);
        }
    }
}

impl Widget for Button {
    fn focusable(&self) -> bool {
        // A disabled button is inert, so it drops out of the Tab order too.
        !self.disabled.get()
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
        let mut style = FlexStyle::new()
            .padding_trbl(self.pad_y, self.pad_x, self.pad_y, self.pad_x)
            .center();
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
        // `min_width` is a *box* floor, but a measured leaf reports content
        // size and Taffy adds the horizontal padding on top — so subtract it
        // here to keep the guard on `min_size` (the measured-leaf invariant,
        // above). A `min_width` shorter than the label is a no-op.
        let mut width = shaped.width.ceil();
        if let Some(mw) = self.min_width {
            width = width.max((mw - 2.0 * self.pad_x).max(0.0));
        }
        Some(Size::new(width, height))
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
        let base_text = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(colors.on_primary);
        let font_size = self
            .font_size
            .unwrap_or(ctx.theme.typography.body.font_size);

        let disabled = self.disabled.get();

        // Hover fade progress, read once. `get()` votes for another frame
        // while the fade is in flight; pressed pins it to 0 (the press color
        // is used directly, reading as instant). A disabled button ignores
        // both — it must read inert regardless of a stale hover animation.
        let hover_t = if self.pressed || disabled {
            0.0
        } else {
            self.hover_anim.as_ref().map_or(0.0, |a| a.get())
        };

        // Background. Endpoints short-circuit so a settled state paints its
        // exact color (float lerp isn't bit-exact at t==1). Disabled wins over
        // every state: the explicit `disabled_bg`, else the normal fill dimmed
        // to half alpha.
        let bg = if disabled {
            self.disabled_bg.as_ref().map(|c| c.get()).unwrap_or(Color {
                a: normal.a * 0.5,
                ..normal
            })
        } else if self.pressed {
            press
        } else if hover_t >= 1.0 {
            hover
        } else if hover_t <= 0.0 {
            normal
        } else {
            normal.lerp(&hover, hover_t)
        };

        // Label color: dimmed to match a disabled fill; otherwise fades toward
        // `hover_text_color` on the same curve when one is set (a text-only
        // link that darkens on hover), else the base color throughout.
        let text_color = if disabled {
            Color {
                a: base_text.a * 0.5,
                ..base_text
            }
        } else {
            match self.hover_text_color.as_ref().map(|c| c.get()) {
                Some(hover_text) if hover_t >= 1.0 => hover_text,
                Some(hover_text) if hover_t > 0.0 => base_text.lerp(&hover_text, hover_t),
                _ => base_text,
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

        if self.focused && !disabled && ctx.focus_visible() {
            let override_color = self.focus_ring_color.as_ref().map(|c| c.get());
            ctx.paint_focus_ring(layout, override_color, self.radius);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        let disabled = self.disabled.get();
        match event {
            // De-activation is processed *even while disabled*. `disabled` gates
            // entering an active state, never leaving one. The tree still routes
            // MouseLeave / FocusLost to a disabled-but-visible widget — hit-test
            // filters on `visible()` and focus dispatch blurs the outgoing
            // widget regardless of `focusable()` — so a button that flips to
            // disabled mid-interaction (its `disabled` is a `Reactive<bool>`,
            // e.g. a form submit) would otherwise strand a stale hover / focus
            // that resurfaces the instant it is re-enabled. These arms always
            // clear.
            WidgetEvent::MouseLeave => {
                self.drive_hover(0.0);
                self.pressed = false;
                EventResult::Consumed
            }
            WidgetEvent::FocusLost => {
                self.focused = false;
                EventResult::Ignored
            }
            // Releasing the press latch is also de-activation: clear it even
            // while disabled (so a press begun before the disable can't stick),
            // but only fire the click when still enabled.
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.pressed {
                    self.pressed = false;
                    if !disabled {
                        self.activate(layout, ctx);
                    }
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            // Everything below newly *enters* an active state or activates the
            // button — all inert while disabled, so nothing latches.
            _ if disabled => EventResult::Ignored,
            WidgetEvent::MouseEnter => {
                self.drive_hover(1.0);
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.pressed = true;
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.focused = true;
                EventResult::Ignored
            }
            // Keyboard activation: Enter triggers click while focused.
            // Browser parity — both Enter and Space activate a button, but
            // Space arrives as `CharInput { ch: ' ' }` (winit routes it
            // through the character pipeline alongside other printable keys).
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            } if self.focused => {
                self.activate(layout, ctx);
                EventResult::Consumed
            }
            WidgetEvent::CharInput { ch: ' ' } if self.focused => {
                self.activate(layout, ctx);
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
    use shroud_reactive::Signal;
    use std::cell::Cell;
    use std::rc::Rc;

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 60.0, 24.0)
    }

    fn left(pos_down: bool) -> WidgetEvent {
        let position = Point::new(5.0, 5.0);
        let button = MouseButton::Left;
        if pos_down {
            WidgetEvent::MouseDown { position, button }
        } else {
            WidgetEvent::MouseUp { position, button }
        }
    }

    // Audit regression (disabled early-return): a button disabled mid-focus
    // must still process FocusLost. The tree blurs the outgoing widget when
    // Tab moves on, and a swallowed blur would leave `focused` true so the
    // ring resurfaces the moment the button is re-enabled.
    #[test]
    fn focus_lost_clears_focus_even_while_disabled() {
        let disabled = Signal::new(false);
        let mut b = Button::new("ok").disabled(disabled);
        let mut ctx = EventContext::new();

        b.event(&WidgetEvent::FocusGained, rect(), &mut ctx);
        assert!(b.is_focused(), "button takes focus while enabled");

        disabled.set(true);
        b.event(&WidgetEvent::FocusLost, rect(), &mut ctx);
        assert!(
            !b.is_focused(),
            "FocusLost must clear focus even while disabled"
        );
    }

    // Same invariant for hover: MouseLeave while disabled must retarget the
    // fade to 0, or the button re-enables looking hovered under a cursor that
    // has moved away.
    #[test]
    fn mouse_leave_clears_hover_even_while_disabled() {
        let disabled = Signal::new(false);
        let mut b = Button::new("ok").disabled(disabled);
        let mut ctx = EventContext::new();

        b.event(&WidgetEvent::MouseEnter, rect(), &mut ctx);
        assert_eq!(
            b.hover_anim.as_ref().map(|a| a.target()),
            Some(1.0),
            "hover fade targets 1 while hovered"
        );

        disabled.set(true);
        b.event(&WidgetEvent::MouseLeave, rect(), &mut ctx);
        assert_eq!(
            b.hover_anim.as_ref().map(|a| a.target()),
            Some(0.0),
            "MouseLeave must retarget the hover fade to 0 even while disabled"
        );
    }

    // The press latch: a press begun while enabled, then disabled, must clear
    // on release rather than sticking (which would show as a stale press color
    // on re-enable).
    #[test]
    fn press_latch_clears_on_release_while_disabled() {
        let disabled = Signal::new(false);
        let mut b = Button::new("ok").disabled(disabled);
        let mut ctx = EventContext::new();

        b.event(&left(true), rect(), &mut ctx);
        assert!(b.pressed, "left press latches while enabled");

        disabled.set(true);
        b.event(&left(false), rect(), &mut ctx);
        assert!(
            !b.pressed,
            "release must clear the press latch even while disabled"
        );
    }

    // The other half of the invariant: while disabled a press must NOT latch
    // in the first place, and must never fire the click.
    #[test]
    fn disabled_button_never_presses_or_activates() {
        let clicks = Rc::new(Cell::new(0u32));
        let c = Rc::clone(&clicks);
        let mut b = Button::new("ok")
            .disabled(true)
            .on_click(move |_| c.set(c.get() + 1));
        let mut ctx = EventContext::new();

        b.event(&left(true), rect(), &mut ctx);
        assert!(!b.pressed, "disabled button does not latch a press");

        b.event(&left(false), rect(), &mut ctx);
        assert_eq!(clicks.get(), 0, "disabled button never fires on_click");
    }

    // Enabled behavior is unchanged: a full press→release fires the click once.
    #[test]
    fn enabled_click_still_fires_once() {
        let clicks = Rc::new(Cell::new(0u32));
        let c = Rc::clone(&clicks);
        let mut b = Button::new("ok").on_click(move |_| c.set(c.get() + 1));
        let mut ctx = EventContext::new();

        b.event(&left(true), rect(), &mut ctx);
        b.event(&left(false), rect(), &mut ctx);
        assert_eq!(clicks.get(), 1, "enabled press→release fires exactly once");
    }
}
