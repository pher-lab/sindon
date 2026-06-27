//! Container widget — a flexbox layout container.

use std::time::Duration;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Lerp, Point, Rect};
use shroud_layout::FlexStyle;
use shroud_reactive::{Animated, Easing, Reactive};

/// Default hover color-transition duration — a short fade (120 ms) so a
/// hoverable row eases in and out of its highlight instead of snapping,
/// matching CSS `transition-colors`. Override per-container with
/// [`Container::hover_transition`] (pass `Duration::ZERO` to disable).
const DEFAULT_HOVER_TRANSITION: Duration = Duration::from_millis(120);

/// Callback type for [`Container::on_context_menu`]. Kept as a type alias
/// so the struct field stays inside `clippy::type_complexity`.
type ContextMenuHandler = Box<dyn FnMut(Point, &mut EventContext)>;

/// Callback type for [`Container::on_press`]. Same shape as
/// [`ContextMenuHandler`] — a left-button press reported with the click
/// position in the subtree's local coordinate space.
type PressHandler = Box<dyn FnMut(Point, &mut EventContext)>;

/// Callback type for [`Container::on_hover_enter`]. Receives the
/// container's own layout rect (viewport coordinates) so the handler can
/// feed it straight into [`LayerAnchor::AnchorRect`](crate::LayerAnchor)
/// to anchor a tooltip / popover to the trigger.
type HoverEnterHandler = Box<dyn FnMut(Rect, &mut EventContext)>;

/// Callback type for [`Container::on_hover_exit`]. The leave carries no
/// position, so the handler just gets the event context (e.g. to pop the
/// tooltip layer it opened on enter).
type HoverExitHandler = Box<dyn FnMut(&mut EventContext)>;

/// A flexbox container widget.
///
/// Containers can have a background color and arrange their children
/// in a row or column via flexbox.
///
/// The background is stored as [`Reactive<Color>`], so the setter accepts
/// either a literal `Color` or a signal-backed source (`Signal<Color>`,
/// `Memo<Color>`, `Reactive::derive(...)`). Dynamic variants are re-read
/// on every paint.
pub struct Container {
    style: FlexStyle,
    background: Option<Reactive<Color>>,
    /// Hover override. `Some` (set via [`Container::hover_background`]) wins
    /// over the theme; `None` with `hoverable == true` falls back to
    /// `theme.hover.bg`.
    hover_bg: Option<Reactive<Color>>,
    /// Whether this container reacts to pointer hover. Off by default —
    /// the vast majority of containers are passive layout boxes, so
    /// enrolling every one in MouseEnter/Leave routing would be wasted
    /// work. Flipped on by [`Container::hoverable`] or by setting an
    /// explicit hover bg.
    hoverable: bool,
    /// Progress of the hover-bg fade, lazily created on the first
    /// MouseEnter/Leave (`None` until then = resting / not hovered). `paint`
    /// lerps the resting background toward the hover background by this
    /// scalar, so reactive endpoints (e.g. a live theme swap) keep tracking
    /// underneath the fade.
    hover_anim: Option<Animated<f32>>,
    /// How long the hover fade takes; `Duration::ZERO` makes it instant.
    hover_transition: Duration,
    radius: f32,
    visible: Reactive<bool>,
    /// Optional right-click handler. When set, `MouseDown { button: Right }`
    /// inside the container's layout rect invokes the handler with the
    /// click position (already translated to the subtree's local coordinate
    /// space by tree dispatch) and the event context — typical use is to
    /// `ctx.push_layer(LayerAnchor::AnchorRect { ... }, ...)` so a context
    /// menu pops up at the cursor.
    on_context_menu: Option<ContextMenuHandler>,
    /// Optional left-press handler. When set, `MouseDown { button: Left }`
    /// inside the container's layout rect invokes the handler with the click
    /// position (already translated to the subtree's local coordinate space)
    /// and the event context, and consumes the event.
    ///
    /// Unlike a [`Button`](crate::Button) — which fires its click on
    /// `MouseUp` after a press latch — this fires on the *press* itself. That
    /// matters for transient UI like an autocomplete list that dismisses when
    /// the owning input loses focus: focus moves (and the dismiss is queued)
    /// on the same `MouseDown`, so a release-based commit would arrive after
    /// the list has already been torn down. A press-based commit is queued in
    /// the same dispatch and survives the teardown. The container does not
    /// need to be focusable, so clicking it never steals keyboard focus.
    on_press: Option<PressHandler>,
    /// Optional hover-enter handler, fired on `MouseEnter` with the
    /// container's own layout rect. Setting it enrolls the container in
    /// hover-event routing *without* turning on the hover-background fade
    /// (that stays gated on [`hoverable`](Self::hoverable)), so a tooltip
    /// trigger doesn't suddenly highlight. Typical use: open a tooltip
    /// layer anchored to the passed rect.
    on_hover_enter: Option<HoverEnterHandler>,
    /// Optional hover-exit handler, fired on `MouseLeave`. Pairs with
    /// [`on_hover_enter`](Self::on_hover_enter) to tear down whatever it
    /// opened.
    on_hover_exit: Option<HoverExitHandler>,
}

impl Container {
    /// Create a column container (vertical stacking). Cross axis is horizontal;
    /// children stretch to the column's full width by default — see
    /// [`FlexStyle::column`] for the cross-axis story.
    pub fn column() -> Self {
        Self {
            style: FlexStyle::new().column(),
            background: None,
            hover_bg: None,
            hoverable: false,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            radius: 0.0,
            visible: Reactive::Static(true),
            on_context_menu: None,
            on_press: None,
            on_hover_enter: None,
            on_hover_exit: None,
        }
    }

    /// Create a row container (horizontal stacking). Cross axis is vertical;
    /// the default `Stretch` makes every child grow to the tallest sibling's
    /// height. For a header that mixes different-height widgets (e.g. a large
    /// title next to a button), chain [`Self::align_center`] to size each
    /// child to its own height and vertically center them — the button's
    /// label will then sit on the same visual baseline as the title.
    pub fn row() -> Self {
        Self {
            style: FlexStyle::new().row(),
            background: None,
            hover_bg: None,
            hoverable: false,
            hover_anim: None,
            hover_transition: DEFAULT_HOVER_TRANSITION,
            radius: 0.0,
            visible: Reactive::Static(true),
            on_context_menu: None,
            on_press: None,
            on_hover_enter: None,
            on_hover_exit: None,
        }
    }

    /// Toggle visibility. `false` gives `display: none` semantics — the
    /// container and its subtree are removed from the layout flow, not
    /// painted, and do not receive events.
    ///
    /// Accepts a literal `bool`, `Signal<bool>`, `Memo<bool>`, or
    /// `Reactive::derive(...)`. The reactive source is re-read every frame,
    /// so wrap expensive closures in a `Memo` if needed.
    pub fn visible(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.visible = v.into();
        self
    }

    /// Set the background color.
    ///
    /// Accepts a literal `Color`, `Signal<Color>`, `Memo<Color>`, or
    /// `Reactive::derive(...)`.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.background = Some(color.into());
        self
    }

    /// Enable pointer-hover styling using the theme's `hover.bg` token.
    ///
    /// Without this (or [`Container::hover_background`]), a container is
    /// inert to pointer enter/leave — the common case, so opt-in keeps
    /// the renderer from re-painting passive layout boxes whenever the
    /// cursor moves through them.
    ///
    /// Combine with [`Container::background`] for the "row that lifts off
    /// surface when the cursor enters" pattern (list items, menu rows).
    pub fn hoverable(mut self) -> Self {
        self.hoverable = true;
        self
    }

    /// Set an explicit background color for the hover state. Implies
    /// [`Container::hoverable`] — calling this alone is enough to opt in.
    pub fn hover_background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.hover_bg = Some(color.into());
        self.hoverable = true;
        self
    }

    /// Set how long the hover background fades when the cursor enters or
    /// leaves. Defaults to a short fade (120 ms); pass [`Duration::ZERO`]
    /// for an instant flip (the pre-animation behavior). Only takes effect
    /// when the container is [`hoverable`](Container::hoverable).
    pub fn hover_transition(mut self, duration: Duration) -> Self {
        self.hover_transition = duration;
        self
    }

    /// Retarget the hover fade, lazily creating the animator on first use.
    /// `to` is `1.0` for fully hovered, `0.0` for resting. The tree only
    /// emits MouseEnter/Leave on an actual hover change, so each call is a
    /// genuine transition (no redundant restarts to guard against).
    fn drive_hover(&mut self, to: f32) {
        self.hover_anim
            .get_or_insert_with(|| Animated::new(0.0, self.hover_transition, Easing::EaseInOut))
            .set(to);
    }

    /// Round the corners of the background fill by `px`. No effect when no
    /// `background` is set, since rounding only applies to the painted rect.
    /// Negative values are clamped to `0.0`; values larger than half of the
    /// shorter side are clamped per-frame in the renderer (no need for the
    /// caller to know the final size).
    pub fn radius(mut self, px: f32) -> Self {
        self.radius = px.max(0.0);
        self
    }

    /// Set padding on all sides.
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, px: f32) -> Self {
        self.style = self.style.gap(px);
        self
    }

    /// Set fixed width.
    pub fn width(mut self, px: f32) -> Self {
        self.style = self.style.width(px);
        self
    }

    /// Set fixed height.
    pub fn height(mut self, px: f32) -> Self {
        self.style = self.style.height(px);
        self
    }

    /// Fill available width.
    pub fn width_full(mut self) -> Self {
        self.style = self.style.width_full();
        self
    }

    /// Fill available height.
    pub fn height_full(mut self) -> Self {
        self.style = self.style.height_full();
        self
    }

    /// Mark the container a scroll-clipping box (`overflow: hidden`), letting a
    /// flex parent size it below its content. See [`FlexStyle::overflow_hidden`].
    ///
    /// Use this on an intermediate `grow(1.0)` container that wraps a
    /// `ScrollView`: without it, the wrapper's automatic minimum size equals its
    /// (overflowing) content, so it balloons to the content height instead of
    /// clamping to the space the parent allocated — and the inner viewport never
    /// becomes scrollable. The `ScrollView` itself already sets this; the
    /// wrapper between it and the height-defining ancestor needs it too.
    pub fn overflow_hidden(mut self) -> Self {
        self.style = self.style.overflow_hidden();
        self
    }

    /// Center children on both axes. See [`FlexStyle::center`] — note that
    /// this collapses children to min-content on the cross axis. For
    /// vertical-only centering in a column without collapsing child width,
    /// use [`Container::justify_center`] instead.
    pub fn center(mut self) -> Self {
        self.style = self.style.center();
        self
    }

    /// Center children on the main axis only. In a column container this
    /// gives vertical centering while leaving children at their natural
    /// width; pair with [`Container::max_width`] for a centered card layout.
    pub fn justify_center(mut self) -> Self {
        self.style = self.style.justify_center();
        self
    }

    /// Center children on the cross axis only. In a column container this
    /// gives horizontal centering. Note the same caveat as [`Container::center`]:
    /// children without explicit cross-axis sizing collapse to min-content.
    /// To center a fixed- or capped-width card, give the *card* a definite
    /// [`Container::width`] plus [`Container::margin_x_auto`] rather than
    /// centering it through this parent setting.
    pub fn align_center(mut self) -> Self {
        self.style = self.style.align_center();
        self
    }

    /// Clamp the container's width — the node grows up to `px` and no further.
    ///
    /// For a centered card, prefer a definite [`Container::width`] plus
    /// [`Container::margin_x_auto`]. Avoid `width_full().max_width(...)`: it
    /// resolves to a percentage width that mis-measures wrapped text height
    /// (see [`FlexStyle::max_width`]).
    pub fn max_width(mut self, px: f32) -> Self {
        self.style = self.style.max_width(px);
        self
    }

    /// Clamp the container's height.
    pub fn max_height(mut self, px: f32) -> Self {
        self.style = self.style.max_height(px);
        self
    }

    /// Horizontally center this container in its parent via auto left/right
    /// margins (CSS `margin-inline: auto`). Pair with [`Self::max_width`] for
    /// a centered, responsive card: the parent stretches it up to `max_width`
    /// and the auto margins absorb the leftover on each side. Preferred over
    /// `width_full() + align_center` parent, which forces a percentage width
    /// that mis-measures wrapped text height. See [`FlexStyle::margin_x_auto`].
    pub fn margin_x_auto(mut self) -> Self {
        self.style = self.style.margin_x_auto();
        self
    }

    /// Grow to fill available space.
    pub fn grow(mut self, factor: f32) -> Self {
        self.style = self.style.grow(factor);
        self
    }

    /// Set the `flex-shrink` factor. Defaults to `1` (the flex default), so a
    /// row's items shrink below their content to fit. Pass `0.0` to pin an
    /// item at its content size — e.g. a brand title or icon that must never
    /// be compressed (and so wrap or ellipsize) when wider siblings crowd the
    /// row. See [`FlexStyle::shrink`].
    pub fn shrink(mut self, factor: f32) -> Self {
        self.style = self.style.shrink(factor);
        self
    }

    /// Set the initial main-axis size (`flex-basis`) in pixels. See
    /// [`FlexStyle::flex_basis`] for the CSS `flex: 1 1 0` use case — pair
    /// with [`Self::grow`] to express "this column takes whatever space is
    /// left over after siblings claim theirs" without expanding to its
    /// content's natural width first.
    pub fn flex_basis(mut self, px: f32) -> Self {
        self.style = self.style.flex_basis(px);
        self
    }

    /// Allow children that overflow the container's main axis to wrap onto
    /// additional lines. Off by default. See [`FlexStyle::flex_wrap`].
    pub fn flex_wrap(mut self, wrap: bool) -> Self {
        self.style = self.style.flex_wrap(wrap);
        self
    }

    /// Register a right-click handler.
    ///
    /// The handler runs on `MouseDown { button: Right }` inside this
    /// container's layout rect. It receives the click position (already
    /// in the subtree's local coordinate space — same frame as
    /// `paint`'s `layout` argument) and the event context, so the
    /// typical body opens a context menu layer anchored at the cursor:
    ///
    /// ```ignore
    /// Container::row().on_context_menu(|pos, ctx| {
    ///     let anchor = Rect::new(pos.x, pos.y, 0.0, 0.0);
    ///     ctx.push_layer(
    ///         LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
    ///             rect: anchor,
    ///             prefer: Placement::Below,
    ///         }),
    ///         ContextMenuRoot::new(),
    ///         |tree, root| {
    ///             tree.add_child(root, MenuItem::new("Rename", |c| { /* … */ c.pop_top_layer(); }));
    ///             tree.add_child(root, MenuItem::new("Delete", |c| { /* … */ c.pop_top_layer(); }));
    ///         },
    ///     );
    /// })
    /// ```
    ///
    /// Setting this also opts the container into pointer events so the
    /// right-click reaches `event` — there is no need to chain
    /// [`Container::hoverable`] separately.
    pub fn on_context_menu(
        mut self,
        handler: impl FnMut(Point, &mut EventContext) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Box::new(handler));
        self
    }

    /// Register a left-press handler, firing on `MouseDown` (not release).
    ///
    /// The handler receives the click position (in the subtree's local
    /// coordinate space, same frame as `paint`'s `layout`) and the event
    /// context; the press is consumed so a parent doesn't also see it.
    ///
    /// Use this for clickable rows that must commit *before* a focus change
    /// queued on the same click can tear them down — e.g. an autocomplete
    /// suggestion that disappears when its input blurs. For ordinary buttons
    /// prefer [`Button`](crate::Button), which commits on release and so
    /// supports press-and-drag-off-to-cancel. Setting this also opts the
    /// container into pointer events, so there's no need to chain
    /// [`Container::hoverable`] for the press alone (do chain it, or set a
    /// hover background, if you also want a hover highlight).
    pub fn on_press(mut self, handler: impl FnMut(Point, &mut EventContext) + 'static) -> Self {
        self.on_press = Some(Box::new(handler));
        self
    }

    /// Register a hover-enter handler, firing on `MouseEnter`.
    ///
    /// The handler receives the container's own layout rect (viewport
    /// coordinates, the same frame as `paint`'s `layout`) and the event
    /// context. The rect feeds straight into
    /// [`LayerAnchor::AnchorRect`](crate::LayerAnchor) so the typical body
    /// opens a tooltip anchored to the trigger:
    ///
    /// ```ignore
    /// Container::row().on_hover_enter(|rect, ctx| {
    ///     ctx.push_layer(
    ///         LayerOptions::tooltip().anchor(LayerAnchor::AnchorRect {
    ///             rect,
    ///             prefer: Placement::Below,
    ///         }),
    ///         tooltip_bubble("Bold"),
    ///         |_tree, _root| {},
    ///     );
    /// })
    /// ```
    ///
    /// Setting this opts the container into hover-event routing **without**
    /// turning on the hover-background fade — that stays gated on
    /// [`hoverable`](Self::hoverable) / [`hover_background`](Self::hover_background),
    /// so a tooltip trigger is not highlighted just for carrying a tip.
    /// The event is not consumed, so a hoverable ancestor still lights up.
    ///
    /// Note that a [`tooltip`](crate::LayerOptions::tooltip) layer is
    /// click-through: it does not steal the `MouseLeave` that drives
    /// [`on_hover_exit`](Self::on_hover_exit). A normal interactive layer
    /// would, and the tip could never dismiss.
    pub fn on_hover_enter(
        mut self,
        handler: impl FnMut(Rect, &mut EventContext) + 'static,
    ) -> Self {
        self.on_hover_enter = Some(Box::new(handler));
        self
    }

    /// Register a hover-exit handler, firing on `MouseLeave`.
    ///
    /// Pairs with [`on_hover_enter`](Self::on_hover_enter) — e.g. pop the
    /// tooltip layer it pushed. The leave carries no position, so the
    /// handler receives only the event context. Like the enter handler this
    /// enrolls the container in hover routing without enabling the hover
    /// fade, and does not consume the event.
    pub fn on_hover_exit(mut self, handler: impl FnMut(&mut EventContext) + 'static) -> Self {
        self.on_hover_exit = Some(Box::new(handler));
        self
    }
}

impl Widget for Container {
    fn style(&self) -> FlexStyle {
        self.style.clone()
    }

    fn visible(&self) -> bool {
        self.visible.get()
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        if self.hoverable {
            let hover = self
                .hover_bg
                .as_ref()
                .map(|c| c.get())
                .unwrap_or(ctx.theme.hover.bg);
            // Resting color: the explicit background, or the hover color at
            // zero alpha so a bg-less row fades in from transparent.
            let resting = self
                .background
                .as_ref()
                .map(|c| c.get())
                .unwrap_or(Color { a: 0.0, ..hover });
            // `get()` votes for another frame while the fade is in flight.
            let t = self.hover_anim.as_ref().map_or(0.0, |a| a.get());
            // Short-circuit the endpoints so a settled state paints its
            // exact color (float lerp isn't bit-exact at t==1, and we want
            // pixel-perfect rest states + deterministic instant transitions).
            let color = if t >= 1.0 {
                hover
            } else if t <= 0.0 {
                resting
            } else {
                resting.lerp(&hover, t)
            };
            if color.a > 0.0 {
                ctx.fill_rect_rounded(layout, color, self.radius);
            }
        } else if let Some(color) = self.background.as_ref().map(|c| c.get()) {
            ctx.fill_rect_rounded(layout, color, self.radius);
        }
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        // Right-click → context menu (independent of hoverable). Consumed
        // so a parent container with its own on_context_menu doesn't fire
        // twice for the same click.
        if let WidgetEvent::MouseDown {
            button: MouseButton::Right,
            position,
        } = event
        {
            if let Some(handler) = &mut self.on_context_menu {
                handler(*position, ctx);
                return EventResult::Consumed;
            }
        }

        // Left press → on_press (independent of hoverable, like the
        // right-click path above). Fires on the press so the commit is
        // queued in the same dispatch that may move focus elsewhere.
        if let WidgetEvent::MouseDown {
            button: MouseButton::Left,
            position,
        } = event
        {
            if let Some(handler) = &mut self.on_press {
                handler(*position, ctx);
                return EventResult::Consumed;
            }
        }

        // Stay inert when not opted in — keeps the no-hover path identical
        // to the pre-A4 behavior (no events consumed, no extra book-keeping).
        // Enrollment is the union of hover *styling* (`hoverable`) and hover
        // *callbacks*: a tooltip trigger sets only a callback and must still
        // see MouseEnter/Leave, but must not get the background fade.
        if !self.hoverable && self.on_hover_enter.is_none() && self.on_hover_exit.is_none() {
            return EventResult::Ignored;
        }
        match event {
            WidgetEvent::MouseEnter => {
                if self.hoverable {
                    self.drive_hover(1.0);
                }
                if let Some(handler) = &mut self.on_hover_enter {
                    handler(layout, ctx);
                }
                // Don't consume — descendants that also care about hover (an
                // inner Button inside a hoverable row) still get to see it.
                EventResult::Ignored
            }
            WidgetEvent::MouseLeave => {
                if self.hoverable {
                    self.drive_hover(0.0);
                }
                if let Some(handler) = &mut self.on_hover_exit {
                    handler(ctx);
                }
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}
