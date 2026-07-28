//! Menu item widget — a single clickable row used by Dropdown popovers,
//! context menus, and any other menu-style layer.
//!
//! Theme-driven: hover highlight uses `theme.hover.bg`, label uses
//! `theme.colors.on_surface`. Apps that need destructive styling (red
//! "Delete" row etc.) can override via [`MenuItem::text_color`].
//!
//! # Keyboard
//!
//! A row is a tab stop, so a menu's rows *are* the tab order of the layer
//! they live in: Tab / Shift+Tab and ↓ / ↑ walk the same ring (the arrows
//! delegate to [`EventContext::advance_focus`]), and Enter / Space activate
//! the focused row exactly as [`Button`](crate::Button) does.
//!
//! This is what makes an open menu reachable at all. Keyboard events are
//! routed to the topmost interactive layer's subtree, so while a menu is up
//! the trigger behind it cannot receive them — if no row could take focus,
//! every keystroke would land nowhere and the menu would be mouse-only.
//! Rows being focusable is also what arms the tree's return-focus-to-trigger
//! path on dismiss (see [`WidgetTree::pop_layer`](crate::tree::WidgetTree)),
//! which only fires when the layer is what held focus.
//!
//! Note this departs from the ARIA menu pattern, where rows carry
//! `tabindex="-1"` and only the arrows move a roving focus. That pattern
//! assumes Tab is *reserved* for closing the menu; here Tab is trapped inside
//! the layer like a modal's, so the two keys agreeing is what a user gets by
//! trying either.

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::focus::FocusDirection;
use crate::interaction::{InteractionState, Release};
use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{AccessAction, AccessNode, AccessRole, Color, Rect, Size};
use sindon_layout::FlexStyle;
use sindon_reactive::Reactive;

/// Click handler for [`MenuItem`]. Same shape as `Button`'s click
/// handler, kept as a type alias so the struct field stays inside
/// `clippy::type_complexity`.
type MenuClickHandler = Box<dyn FnMut(&mut EventContext)>;

/// Horizontal padding (Tailwind `px-3`) — declared in `style` so it grows the
/// box, and re-used in `paint` to inset the label so it does not hug the left
/// edge. The two must stay in sync.
const H_PADDING: f32 = 12.0;
/// Vertical padding (Tailwind `py-1.5`).
const V_PADDING: f32 = 6.0;

/// A single row in a menu-style layer (dropdown popover, context menu).
///
/// Left-aligned label, theme-driven hover highlight, click fires the
/// supplied handler. The handler receives the [`EventContext`] so it can
/// enqueue `pop_top_layer` / `push_layer` / focus changes — the typical
/// shape is "do something, then dismiss":
///
/// ```
/// # use sindon_widgets::MenuItem;
/// let delete = MenuItem::new("Delete", |ctx| {
///     // ... do work ...
///     ctx.pop_top_layer();
/// });
/// ```
pub struct MenuItem {
    label: String,
    on_click: Option<MenuClickHandler>,
    text_color: Option<Reactive<Color>>,
    /// Gate the row inert (Tailwind `disabled:opacity-40`). Reactive so a menu
    /// can bind it to app state (e.g. "Export all notes" off while the note
    /// list is empty) with no event to flip it. Dims the label and swallows
    /// activation — the clearing discipline lives in [`InteractionState`].
    disabled: Reactive<bool>,
    /// Hover / press / focus flags — see [`InteractionState`].
    state: InteractionState,
}

impl MenuItem {
    /// Create a menu item with the given label and click handler. The
    /// handler runs on `MouseUp` after a `MouseDown` on the same row
    /// (matches `Button`'s activation semantics — drag-off cancels).
    pub fn new(
        label: impl Into<String>,
        on_click: impl FnMut(&mut EventContext) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Some(Box::new(on_click)),
            text_color: None,
            disabled: Reactive::Static(false),
            state: InteractionState::default(),
        }
    }

    /// Override the label color. Defaults to `theme.colors.on_surface`.
    /// Useful for destructive rows (e.g. red "Delete").
    pub fn text_color(mut self, color: impl Into<Reactive<Color>>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    /// Gate the row on a disabled state (reactive), matching Tailwind
    /// `disabled:opacity-40`: the label dims to half alpha, hover is suppressed,
    /// and clicks no longer fire the handler. Accepts a literal `bool` or a
    /// `Signal<bool>` the menu binds to app state. The `InteractionState`
    /// discipline still lets a row disabled mid-press clear its latch cleanly.
    pub fn disabled(mut self, v: impl Into<Reactive<bool>>) -> Self {
        self.disabled = v.into();
        self
    }

    /// Whether the row currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.state.focused
    }

    /// Fire the click handler. The one activation path behind mouse-release,
    /// Enter / Space, and a screen reader's press — each caller does its own
    /// `disabled` gating first.
    fn activate(&mut self, ctx: &mut EventContext) {
        if let Some(handler) = &mut self.on_click {
            handler(ctx);
        }
    }
}

impl Widget for MenuItem {
    fn focusable(&self) -> bool {
        // A disabled row is inert, so it drops out of the Tab order too —
        // matching `Button`, and keeping ↓ from parking on a row that will
        // not fire.
        !self.disabled.get()
    }

    fn accessibility(&self) -> Option<AccessNode> {
        Some(
            AccessNode::new(AccessRole::MenuItem)
                .name(self.label.clone())
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
        // A screen reader's "press" is the third activation route, joining
        // mouse-release and Enter/Space on the shared `activate` path — and it
        // is inert while disabled, exactly like the other two.
        if action != AccessAction::Click || self.disabled.get() {
            return EventResult::Ignored;
        }
        self.activate(ctx);
        EventResult::Consumed
    }

    fn style(&self) -> FlexStyle {
        // Measured-leaf invariant (see `Button::style`): no style `min_size` on
        // a widget that also reports its size via `measure`, or Taffy
        // over-counts the content height of a content-hugging ancestor (the
        // centered-card dead-space bug). The 28px minimum row height lives in
        // `measure` instead — see the height floor there.
        FlexStyle::new().padding_trbl(V_PADDING, H_PADDING, V_PADDING, H_PADDING)
    }

    fn measure(&self, available_width: Option<f32>, ctx: &mut MeasureContext) -> Option<Size> {
        let font_size = ctx.theme.typography.body.font_size;
        if self.label.is_empty() {
            return Some(Size::new(0.0, font_size));
        }
        let line_height = font_size * 1.2;
        let natural = ctx
            .text_engine
            .measure_text(&self.label, font_size, line_height, None);
        let (shaped_w, shaped_h) = match available_width {
            Some(aw) if natural.0 > aw => {
                ctx.text_engine
                    .measure_text(&self.label, font_size, line_height, Some(aw))
            }
            _ => natural,
        };
        // Floor the content height to the old `min_height(28)` border box. That
        // minimum used to live in `style().min_size`, but a measured leaf must
        // not carry one (see `MenuItem::style`). Taffy adds the 12px vertical
        // padding on top of this content height, so the content floor is
        // `28 − 12 = 16`; without it, short rows at small font scales would dip
        // below their historical 28px once the style `min_size` is gone.
        let min_content_height = 28.0 - 12.0; // old min_height − vertical padding
        Some(Size::new(
            shaped_w.ceil(),
            shaped_h.max(font_size).max(min_content_height).ceil(),
        ))
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let hover_bg = ctx.theme.hover.bg;
        let default_text = ctx.theme.colors.on_surface;
        let font_size = ctx.theme.typography.body.font_size;
        let disabled = self.disabled.get();
        let mut text_color = self
            .text_color
            .as_ref()
            .map(|c| c.get())
            .unwrap_or(default_text);
        // Greyed-out label (Tailwind `disabled:opacity-40`); half-alpha reads on
        // any surface, matching `Button`'s default disabled dim.
        if disabled {
            text_color = Color {
                a: text_color.a * 0.5,
                ..text_color
            };
        }

        // The active row — the one Enter would fire, or a click would. A menu
        // marks it by filling the row rather than ringing it: that is what every
        // native menu does, and a ring inset into a 28px row reads as clutter.
        // So keyboard focus reuses the hover fill instead of `paint_focus_ring`.
        //
        // Gated on `focus_visible` like any ring, which is also what keeps the
        // fill from lying: pointer focus (a click that is about to fire the row
        // anyway) leaves it alone, and a context menu popped up under the cursor
        // still starts with nothing pre-highlighted — the same promise
        // `WidgetTree::resync_hover` makes for hover.
        let focus_active = self.state.focused && ctx.focus_visible();
        // Suppress the fill while disabled — a row disabled while the
        // pointer sits on it keeps `hovered` (no event flips a reactive signal),
        // so gate on `disabled` too rather than relying on the flag alone.
        let bg = if (self.state.hovered || focus_active) && !disabled {
            hover_bg
        } else {
            Color::TRANSPARENT
        };
        if bg.a > 0.0 {
            ctx.fill_rect(layout, bg);
        }

        let line_height = font_size * 1.2;
        // Inset the label by the horizontal padding declared in `style` — the
        // laid-out `layout` is the border box, so painting at `origin.x` would
        // let the label hug the left edge and ignore the `px-3` gutter.
        let max_w = (layout.size.width - 2.0 * H_PADDING).max(0.0);
        let shaped = ctx
            .text_engine
            .shape_text(&self.label, font_size, line_height, Some(max_w));
        let text_x = layout.origin.x + H_PADDING;
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

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        let disabled = self.disabled.get();
        match event {
            // Clearing transitions run even while disabled (see
            // [`InteractionState`]) so a row disabled mid-hover/press does not
            // strand a stale flag that resurfaces on re-enable.
            WidgetEvent::MouseLeave => {
                self.state.leave();
                EventResult::Consumed
            }
            WidgetEvent::FocusLost => {
                self.state.focus_lost();
                EventResult::Ignored
            }
            WidgetEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => match self.state.release(disabled) {
                Release::Fire => {
                    self.activate(ctx);
                    EventResult::Consumed
                }
                Release::Cancelled => EventResult::Consumed,
                Release::Idle => EventResult::Ignored,
            },
            // Latching transitions (enter / press) are inert while disabled.
            _ if disabled => EventResult::Ignored,
            WidgetEvent::MouseEnter => {
                self.state.enter(disabled);
                EventResult::Consumed
            }
            WidgetEvent::FocusGained => {
                self.state.focus_gained(disabled);
                EventResult::Ignored
            }
            // Keyboard activation, matching `Button`: Enter arrives as a named
            // key, Space through the character pipeline.
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            } if self.state.focused => {
                self.activate(ctx);
                EventResult::Consumed
            }
            WidgetEvent::CharInput { ch: ' ' } if self.state.focused => {
                self.activate(ctx);
                EventResult::Consumed
            }
            // ↓ / ↑ are a menu's native way between rows. They mean exactly what
            // Tab / Shift+Tab mean here — a menu's rows are the tab order of its
            // layer — so hand them to the tree rather than deriving a sibling
            // index this row cannot see (see `EventContext::advance_focus`).
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowDown),
            } if self.state.focused => {
                ctx.advance_focus(FocusDirection::Forward);
                EventResult::Consumed
            }
            WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::ArrowUp),
            } if self.state.focused => {
                ctx.advance_focus(FocusDirection::Backward);
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                self.state.press(disabled);
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}
