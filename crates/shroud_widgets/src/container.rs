//! Container widget — a flexbox layout container.

use std::time::Duration;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::paint::PaintContext;
use crate::widget::Widget;
use shroud_core::{Color, Lerp, Point, Rect};
use shroud_layout::{Align, FlexStyle, Justify};
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

/// Callback type for [`Container::on_press_rect`]. Like [`PressHandler`] but
/// reports the container's own layout rect (for anchoring a popover to the
/// trigger) instead of the click point — the press-time counterpart to
/// [`HoverEnterHandler`].
type PressRectHandler = Box<dyn FnMut(Rect, &mut EventContext)>;

/// Callback type for [`Container::on_hover_enter`]. Receives the
/// container's own layout rect (viewport coordinates) so the handler can
/// feed it straight into [`LayerAnchor::AnchorRect`](crate::LayerAnchor)
/// to anchor a tooltip / popover to the trigger.
type HoverEnterHandler = Box<dyn FnMut(Rect, &mut EventContext)>;

/// Callback type for [`Container::on_hover_exit`]. The leave carries no
/// position, so the handler just gets the event context (e.g. to pop the
/// tooltip layer it opened on enter).
type HoverExitHandler = Box<dyn FnMut(&mut EventContext)>;

/// One edge's border: stroke thickness in px paired with its (reactive)
/// color. Backs the single-side [`Container::border_top`] etc. builders,
/// which draw a sharp line along one layout edge — the flexbox-native way
/// to reproduce a Tailwind `border-r` / `border-b` divider without the
/// four-sided [`Container::border`].
type SideBorder = (f32, Reactive<Color>);

/// Normalize a side-border builder argument: `width <= 0.0` clamps to `None`
/// (no line), matching the four-sided [`Container::border`] contract.
fn side_border(width: f32, color: impl Into<Reactive<Color>>) -> Option<SideBorder> {
    if width > 0.0 {
        Some((width, color.into()))
    } else {
        None
    }
}

/// A drop shadow cast behind a container's box (CSS `box-shadow`).
///
/// Set via [`Container::shadow`]. The offset shifts the shadow relative to the
/// box, `blur` softens its edge, and `spread` grows (positive) or shrinks
/// (negative) the silhouette before blurring — the same four numbers as a CSS
/// `box-shadow`. The color is [`Reactive`] so a shadow can deepen on a dark
/// theme, though the common case is a static translucent black.
struct BoxShadow {
    offset_x: f32,
    offset_y: f32,
    blur: f32,
    spread: f32,
    color: Reactive<Color>,
}

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
    /// Border stroke width in px. `0.0` (the default) draws no border. Set via
    /// [`Container::border`]; the stroke hugs the inside of the layout edge and
    /// rounds its corners with [`radius`](Self::radius), reusing the same SDF
    /// path as [`Input`](crate::Input)'s frame.
    border_width: f32,
    /// Border color, re-read every paint like [`background`](Self::background).
    /// `None` when no border is set.
    border_color: Option<Reactive<Color>>,
    /// Per-edge single-side borders (top, right, bottom, left). Each is a
    /// sharp line drawn along that layout edge, independent of the four-sided
    /// [`border`](Self::border_width) — the divider idiom (`border-r` /
    /// `border-b`). `None` = that edge has no line. Set via
    /// [`border_top`](Self::border_top) etc.
    border_top: Option<SideBorder>,
    border_right: Option<SideBorder>,
    border_bottom: Option<SideBorder>,
    border_left: Option<SideBorder>,
    /// Optional drop shadow, painted behind the background fill. `None` (the
    /// default) casts no shadow. Set via [`Container::shadow`] — the modal /
    /// card `shadow-xl` elevation the flat scrim couldn't convey.
    shadow: Option<BoxShadow>,
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
    /// Optional left-press handler fired on `MouseDown` with the container's
    /// own layout rect (rather than the click point), for anchoring a popover
    /// to the trigger itself. See [`Container::on_press_rect`].
    on_press_rect: Option<PressRectHandler>,
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
            border_width: 0.0,
            border_color: None,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            shadow: None,
            visible: Reactive::Static(true),
            on_context_menu: None,
            on_press: None,
            on_press_rect: None,
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
            border_width: 0.0,
            border_color: None,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            shadow: None,
            visible: Reactive::Static(true),
            on_context_menu: None,
            on_press: None,
            on_press_rect: None,
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

    /// Draw a `width`-px border around the container in `color`.
    ///
    /// The stroke hugs the inside of the layout edge and rounds its corners
    /// with [`radius`](Self::radius) — pair them for the ubiquitous
    /// `border border-gray-300 rounded-lg` card / panel / divider look. The
    /// color is [`Reactive`], re-read every paint like
    /// [`background`](Self::background), so a border can track a live theme
    /// swap. `width <= 0.0` clamps to `0.0` (no border), the default.
    ///
    /// Unlike the fill, a border paints even when no `background` is set, so a
    /// transparent box can carry just an outline.
    pub fn border(mut self, width: f32, color: impl Into<Reactive<Color>>) -> Self {
        self.border_width = width.max(0.0);
        self.border_color = Some(color.into());
        self
    }

    /// Draw a `width`-px line along the container's **top** edge in `color`
    /// (CSS `border-top`). Independent of the four-sided [`border`](Self::border):
    /// a sharp line hugging one layout edge, the flexbox-native replacement for
    /// the "1px divider Container" hack. The color is [`Reactive`], re-read
    /// every paint. `width <= 0.0` draws nothing.
    pub fn border_top(mut self, width: f32, color: impl Into<Reactive<Color>>) -> Self {
        self.border_top = side_border(width, color);
        self
    }

    /// Draw a `width`-px line along the container's **right** edge in `color`
    /// (CSS `border-right`) — the sidebar `border-r` idiom. See
    /// [`border_top`](Self::border_top).
    pub fn border_right(mut self, width: f32, color: impl Into<Reactive<Color>>) -> Self {
        self.border_right = side_border(width, color);
        self
    }

    /// Draw a `width`-px line along the container's **bottom** edge in `color`
    /// (CSS `border-bottom`) — the section-separator `border-b` idiom. See
    /// [`border_top`](Self::border_top).
    pub fn border_bottom(mut self, width: f32, color: impl Into<Reactive<Color>>) -> Self {
        self.border_bottom = side_border(width, color);
        self
    }

    /// Draw a `width`-px line along the container's **left** edge in `color`
    /// (CSS `border-left`). See [`border_top`](Self::border_top).
    pub fn border_left(mut self, width: f32, color: impl Into<Reactive<Color>>) -> Self {
        self.border_left = side_border(width, color);
        self
    }

    /// Cast a drop shadow behind the container (CSS `box-shadow`).
    ///
    /// The four numbers mirror CSS: `offset_x` / `offset_y` shift the shadow
    /// (positive `offset_y` drops it below the box), `blur` softens the edge
    /// over that many pixels, and `spread` grows (positive) or shrinks
    /// (negative) the silhouette before blurring. The shadow rounds with the
    /// container's [`radius`](Self::radius) and is painted **behind** the fill
    /// and children, so an opaque background covers the interior and only the
    /// blurred halo peeking past the box shows.
    ///
    /// This is the elevation cue a flat scrim can't give a modal or menu. A
    /// Tailwind `shadow-xl` reads well as a single soft shadow, e.g.
    /// `shadow(0.0, 12.0, 24.0, -4.0, Color::rgba(0.0, 0.0, 0.0, 0.18))`;
    /// prefer [`elevation`](Self::elevation) for the tuned presets. The color
    /// is [`Reactive`], re-read every paint like [`background`](Self::background).
    pub fn shadow(
        mut self,
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        spread: f32,
        color: impl Into<Reactive<Color>>,
    ) -> Self {
        self.shadow = Some(BoxShadow {
            offset_x,
            offset_y,
            blur,
            spread,
            color: color.into(),
        });
        self
    }

    /// Cast a preset drop shadow for a `level` of elevation (1–4), the
    /// ergonomic form of [`shadow`](Self::shadow) for the common Material /
    /// Tailwind card tiers. Higher levels sit "further" off the surface with a
    /// larger, softer, more offset shadow:
    ///
    /// - `1` — resting card (`shadow-sm`/`shadow`): subtle.
    /// - `2` — raised card / dropdown (`shadow-md`).
    /// - `3` — popover / menu (`shadow-lg`).
    /// - `4` — modal dialog (`shadow-xl`): the deepest.
    ///
    /// Levels clamp to `1..=4`; `0` or less casts no shadow. All presets use a
    /// translucent black tuned to read on both light and dark surfaces.
    pub fn elevation(self, level: u8) -> Self {
        // (offset_y, blur, spread, alpha) per tier — offset_x stays 0 (shadows
        // fall straight down, as in Material / Tailwind). Blur/offset grow with
        // the tier; the slight negative spread keeps the halo from splaying too
        // wide at the sides, matching Tailwind's `shadow-lg`/`xl`.
        let (dy, blur, spread, alpha) = match level {
            0 => return self,
            1 => (1.0, 3.0, 0.0, 0.12),
            2 => (4.0, 8.0, -1.0, 0.14),
            3 => (8.0, 16.0, -2.0, 0.16),
            _ => (12.0, 24.0, -4.0, 0.18),
        };
        self.shadow(0.0, dy, blur, spread, Color::rgba(0.0, 0.0, 0.0, alpha))
    }

    /// Set padding on all sides.
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self
    }

    /// Set padding per axis: `x` on the left and right, `y` on the top and
    /// bottom (CSS `padding: y x`). The ubiquitous Tailwind `px-* py-*` idiom
    /// — a section header at `px-6 py-4`, a chip at `px-2 py-1` — that the
    /// uniform [`padding`](Self::padding) can't express. Negative values clamp
    /// to `0.0`. For fully independent edges use [`padding_trbl`](Self::padding_trbl).
    pub fn padding_xy(mut self, x: f32, y: f32) -> Self {
        let (x, y) = (x.max(0.0), y.max(0.0));
        self.style = self.style.padding_trbl(y, x, y, x);
        self
    }

    /// Set padding per edge: `top`, `right`, `bottom`, `left` (CSS shorthand
    /// order). The most general form — reach for [`padding`](Self::padding) or
    /// [`padding_xy`](Self::padding_xy) first, and use this only when the edges
    /// genuinely differ (e.g. a panel insetting its content asymmetrically).
    /// Negative values clamp to `0.0`.
    pub fn padding_trbl(mut self, top: f32, right: f32, bottom: f32, left: f32) -> Self {
        self.style =
            self.style
                .padding_trbl(top.max(0.0), right.max(0.0), bottom.max(0.0), left.max(0.0));
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

    /// Distribute children along the main axis (CSS `justify-content`) — the
    /// general form of [`Container::justify_center`]. Use
    /// [`Justify::SpaceBetween`] for the ubiquitous "title left, actions right"
    /// header row, or `Start` / `End` to pin the group to one end. See
    /// [`Justify`] for the full range.
    pub fn justify(mut self, justify: Justify) -> Self {
        self.style = self.style.justify(justify);
        self
    }

    /// Align children on the cross axis (CSS `align-items`) — the general form
    /// of [`Container::align_center`]. `Start` / `Center` / `End` size each
    /// child to its own cross extent; the default `Stretch` fills the cross
    /// axis. See [`Align`] and the min-content caveat on
    /// [`Container::align_center`].
    pub fn align(mut self, align: Align) -> Self {
        self.style = self.style.align(align);
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
    ///             align: HAlign::Start,
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

    /// Register a left-press handler that receives the container's own layout
    /// rect (viewport coordinates, the same frame as `paint`'s `layout`)
    /// instead of the click point — the press-time counterpart to
    /// [`on_hover_enter`](Self::on_hover_enter).
    ///
    /// The rect feeds straight into
    /// [`LayerAnchor::AnchorRect`](crate::LayerAnchor), so a menu button can
    /// open a popover anchored to *itself*: dropped under the button and,
    /// with [`HAlign::End`](crate::HAlign), flush to its right edge (CSS
    /// `right-0 top-full`). [`on_press`](Self::on_press) only hands back the
    /// cursor point, which can anchor a menu at the cursor but not to the
    /// trigger's box.
    ///
    /// ```ignore
    /// Container::row().hoverable().on_press_rect(|rect, ctx| {
    ///     ctx.push_layer(
    ///         LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
    ///             rect,
    ///             prefer: Placement::Below,
    ///             align: HAlign::End, // right-0
    ///         }),
    ///         menu_panel(),
    ///         |tree, root| { /* populate */ },
    ///     );
    /// })
    /// ```
    ///
    /// Fires on `MouseDown` and consumes the event, like
    /// [`on_press`](Self::on_press). May be combined with `on_press`; both
    /// fire on the same press. The rect is layer-local inside a layer, and
    /// [`push_layer`](crate::EventContext::push_layer) translates the anchor
    /// to viewport space — so the button lands correctly even nested in a
    /// modal or popover.
    pub fn on_press_rect(mut self, handler: impl FnMut(Rect, &mut EventContext) + 'static) -> Self {
        self.on_press_rect = Some(Box::new(handler));
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
    ///             align: HAlign::Start,
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
        // Drop shadow first, so it sits behind the fill and children. The
        // caster box is the layout rect offset by (offset_x, offset_y) and
        // inflated by `spread`; its radius grows with the spread so a rounded
        // card keeps a concentric rounded halo.
        if let Some(sh) = &self.shadow {
            let color = sh.color.get();
            if color.a > 0.0 && sh.blur > 0.0 {
                let x = layout.origin.x + sh.offset_x - sh.spread;
                let y = layout.origin.y + sh.offset_y - sh.spread;
                let w = layout.size.width + 2.0 * sh.spread;
                let h = layout.size.height + 2.0 * sh.spread;
                // A large negative spread can shrink the box away entirely —
                // nothing to cast then.
                if w > 0.0 && h > 0.0 {
                    let radius = if self.radius > 0.0 {
                        (self.radius + sh.spread).max(0.0)
                    } else {
                        0.0
                    };
                    ctx.fill_shadow(Rect::new(x, y, w, h), color, radius, sh.blur);
                }
            }
        }

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

    /// Borders paint *after* the children so a full-bleed child background
    /// can't obscure them. A container's border is a frame around the whole
    /// box — the intuitive behavior (and how CSS borders sit outside the
    /// content box) — but our flex model doesn't inset children by the border
    /// width, so a stretched child with its own background (e.g. a full-width
    /// `ScrollView`) reaches the very edge and would overpaint a border drawn
    /// pre-children.
    fn paint_post_children(&self, layout: Rect, ctx: &mut PaintContext) {
        // Four-sided border stroke, over the fill and children so it reads on
        // top of a background and on its own for a transparent outlined box.
        // Reuses the same rounded-SDF path as Input's frame.
        if self.border_width > 0.0 {
            if let Some(color) = self.border_color.as_ref().map(|c| c.get()) {
                ctx.stroke_rect_rounded(layout, color, self.radius, self.border_width);
            }
        }

        // Single-side borders (dividers). Each is a sharp filled line hugging
        // one layout edge, painted after the four-sided stroke so an explicit
        // divider always reads on top. Corners are square — a one-sided line
        // has no rounding to reconcile with `radius`.
        let (x, y, w, h) = (
            layout.origin.x,
            layout.origin.y,
            layout.size.width,
            layout.size.height,
        );
        if let Some((t, color)) = &self.border_top {
            ctx.fill_rect(Rect::new(x, y, w, *t), color.get());
        }
        if let Some((t, color)) = &self.border_bottom {
            ctx.fill_rect(Rect::new(x, y + h - t, w, *t), color.get());
        }
        if let Some((t, color)) = &self.border_left {
            ctx.fill_rect(Rect::new(x, y, *t, h), color.get());
        }
        if let Some((t, color)) = &self.border_right {
            ctx.fill_rect(Rect::new(x + w - t, y, *t, h), color.get());
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

        // Left press → on_press / on_press_rect (independent of hoverable,
        // like the right-click path above). Fires on the press so the commit
        // is queued in the same dispatch that may move focus elsewhere.
        // `on_press` gets the click point; `on_press_rect` gets the
        // container's own layout rect (to anchor a popover to the trigger).
        // Both fire when both are set.
        if let WidgetEvent::MouseDown {
            button: MouseButton::Left,
            position,
        } = event
        {
            let mut handled = false;
            if let Some(handler) = &mut self.on_press {
                handler(*position, ctx);
                handled = true;
            }
            if let Some(handler) = &mut self.on_press_rect {
                handler(layout, ctx);
                handled = true;
            }
            if handled {
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
