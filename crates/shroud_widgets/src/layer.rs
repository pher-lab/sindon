//! Overlay layer primitives — popups painted on top of the main widget tree.
//!
//! A *layer* is an independent widget subtree (with its own root) painted
//! over the main tree and given event priority. Layers are the building
//! block for modal dialogs, dropdowns, context menus, tooltips, and any
//! other UI element that escapes the normal flex flow.
//!
//! Layers are managed by [`WidgetTree`](crate::tree::WidgetTree) via the
//! `push_layer` / `pop_layer` primitives; from event handlers, see
//! [`EventContext::push_layer`](crate::event::EventContext::push_layer).
//!
//! Three anchors are implemented:
//!
//! - [`LayerAnchor::ViewportCenter`] — modal dialogs.
//! - [`LayerAnchor::AnchorRect`] — popovers attached to a trigger rect
//!   (dropdowns, context menus), horizontally aligned via [`HAlign`] and
//!   vertically placed via [`Placement`].
//! - [`LayerAnchor::Viewport`] — a fixed corner/edge of the viewport plus a
//!   pixel offset, for CSS-`absolute`-style placement such as a top-center
//!   toast/banner. `LayerAnchor` stays `#[non_exhaustive]` so further anchor
//!   kinds can be added later without a break.

use shroud_core::{Color, Rect};

/// Preferred vertical placement of an [`LayerAnchor::AnchorRect`] popover.
///
/// The actual placement may flip when the preferred side does not fit in
/// the viewport — see the placement math in
/// [`WidgetTree::compute_layout_with_measure`](crate::tree::WidgetTree::compute_layout_with_measure).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Place the popover directly below the trigger rect (top edge meets
    /// the trigger's bottom edge). The default — matches dropdown UX.
    #[default]
    Below,
    /// Place the popover directly above the trigger rect (bottom edge
    /// meets the trigger's top edge).
    Above,
    /// Try [`Self::Below`] first; flip to [`Self::Above`] if the popover
    /// would overflow the viewport bottom. Identical to `Below` when there
    /// is room either way.
    Auto,
}

/// Horizontal alignment for [`LayerAnchor::AnchorRect`] and
/// [`LayerAnchor::Viewport`].
///
/// For `AnchorRect` the alignment is relative to the *trigger rect*; for
/// `Viewport` it is relative to the *viewport*. Either way the resolved
/// x-coordinate is clamped so the layer stays on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    /// Left edges align. `AnchorRect`: the popover's left edge meets the
    /// trigger's left edge (CSS `left-0`) — the default dropdown/menu drop.
    /// `Viewport`: pin to the left viewport edge.
    #[default]
    Start,
    /// Centers align. `AnchorRect`: the popover is centered over the
    /// trigger. `Viewport`: horizontally centered in the viewport.
    Center,
    /// Right edges align. `AnchorRect`: the popover's right edge meets the
    /// trigger's right edge (CSS `right-0`), opening leftward from the
    /// trigger. `Viewport`: pin to the right viewport edge.
    End,
}

/// Vertical position for a [`LayerAnchor::Viewport`] layer, relative to the
/// viewport.
///
/// Rect-anchored layers use [`Placement`] instead, which adds the `Auto`
/// flip; a viewport-pinned layer has no trigger to flip around, so it takes a
/// plain top / center / bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    /// Pin to the top viewport edge.
    #[default]
    Start,
    /// Vertically centered in the viewport.
    Center,
    /// Pin to the bottom viewport edge.
    End,
}

/// Where a layer is placed relative to the viewport or another widget.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum LayerAnchor {
    /// Center the layer's natural-size root inside the viewport. The
    /// standard modal-dialog placement. Equivalent to
    /// `Viewport { h: HAlign::Center, v: VAlign::Center, offset: (0.0, 0.0) }`,
    /// kept as a named shorthand for the common modal case.
    ViewportCenter,
    /// Anchor the layer to a trigger rect. Horizontal placement follows
    /// `align` (left edges by default, [`HAlign::End`] for CSS `right-0`);
    /// vertical placement follows `prefer`, with `Auto` flipping above when
    /// below would overflow. Both coordinates are clamped so the popover
    /// stays inside the viewport.
    ///
    /// `rect` is in the coordinate space of the handler that pushes the
    /// layer. From the main tree that is viewport space; from inside another
    /// layer it is that layer's local space, and
    /// [`EventContext::push_layer`](crate::event::EventContext::push_layer)
    /// translates it to viewport via
    /// [`EventContext::layer_offset`](crate::event::EventContext::layer_offset).
    /// Either way a widget can just pass the `layout` rect it received in
    /// `event` (or a cursor position from `on_context_menu`) unchanged.
    ///
    /// Typical use: a [`Dropdown`](crate::Dropdown) reads its own layout
    /// rect inside its click handler and pushes a popover layer with this
    /// anchor variant — and it now lands correctly even when the dropdown
    /// itself lives inside a modal or popover. A menu button anchored to
    /// *itself* gets its rect from
    /// [`Container::on_press_rect`](crate::Container::on_press_rect).
    AnchorRect {
        rect: Rect,
        prefer: Placement,
        align: HAlign,
    },
    /// Pin the layer to a fixed viewport corner/edge plus a pixel offset —
    /// CSS-`absolute`/`fixed`-style placement with no trigger. A top-center
    /// banner (`top-2 left-1/2 -translate-x-1/2`) is
    /// `Viewport { h: HAlign::Center, v: VAlign::Start, offset: (0.0, 8.0) }`.
    ///
    /// Unlike [`Self::AnchorRect`], the position is resolved purely against
    /// the viewport and is independent of the pushing layer's own offset (so
    /// [`push_layer`](crate::event::EventContext::push_layer) never
    /// translates it). `offset` nudges the resolved corner (positive x =
    /// right, positive y = down); the final position is clamped to keep the
    /// layer on screen.
    Viewport {
        h: HAlign,
        v: VAlign,
        offset: (f32, f32),
    },
}

/// Configuration for a pushed layer.
///
/// Use [`Self::modal`] for the typical "darkened backdrop, dismissable" preset,
/// or [`Self::popover`] for a chrome-less layer (dropdowns / menus). Both can
/// be tuned further via the builder methods.
#[derive(Debug, Clone)]
pub struct LayerOptions {
    /// Placement strategy.
    pub anchor: LayerAnchor,
    /// Optional full-viewport tint painted behind the layer content.
    /// `None` leaves the underlying UI fully visible (typical for popovers).
    pub scrim: Option<Color>,
    /// Pointer-down on the scrim / outside the layer's rect closes the
    /// layer. Either way the click is swallowed — while a layer is up,
    /// the main tree never sees pointer or keyboard events, matching
    /// browser popover behavior.
    pub dismiss_on_outside_click: bool,
    /// `Escape` closes the layer when it is the topmost.
    pub dismiss_on_escape: bool,
    /// Suppress all app-level keyboard shortcuts while this layer is on
    /// top — including
    /// [`ShortcutScope::Global`](crate::shortcut::ShortcutScope::Global)
    /// bindings.
    ///
    /// Default `false`: app-level `Global` shortcuts (lock, quit, …)
    /// still fire through a modal, and `WhenNoTextInput` shortcuts fire
    /// unless an input *inside* the modal currently has focus. Opt in to
    /// `true` for "panic" sheets where every keystroke must reach the
    /// dialog (e.g. a confirm-delete prompt where Ctrl+L should not yank
    /// the user out mid-confirmation).
    pub block_shortcuts: bool,
    /// Whether this layer participates in event routing. `true` (the
    /// default) is the normal modal/popover behavior: while the layer is
    /// topmost it captures all pointer and keyboard input and the main
    /// tree sees nothing.
    ///
    /// `false` makes the layer **click-through** — it still lays out and
    /// paints on top, but event dispatch skips it entirely, routing
    /// pointer/keyboard input to the topmost *interactive* layer (or the
    /// main tree). This is what a tooltip needs: a paint-only overlay that
    /// must not steal the `MouseLeave` from its trigger, otherwise the
    /// trigger never learns the cursor left and the tip can never dismiss.
    /// A non-interactive layer also never dismisses on outside-click or
    /// Escape (there is nothing to dismiss it *to* — the caller pops it).
    /// See [`Self::tooltip`].
    pub interactive: bool,
}

impl LayerOptions {
    /// Default modal preset: semi-transparent black scrim, dismiss on
    /// outside-click and Escape, centered in the viewport.
    pub fn modal() -> Self {
        Self {
            anchor: LayerAnchor::ViewportCenter,
            scrim: Some(Color::rgba(0.0, 0.0, 0.0, 0.5)),
            dismiss_on_outside_click: true,
            dismiss_on_escape: true,
            block_shortcuts: false,
            interactive: true,
        }
    }

    /// Chrome-less popover preset: no scrim, dismiss on outside-click and
    /// Escape. Used for dropdowns / context menus once those anchors land.
    pub fn popover() -> Self {
        Self {
            anchor: LayerAnchor::ViewportCenter,
            scrim: None,
            dismiss_on_outside_click: true,
            dismiss_on_escape: true,
            block_shortcuts: false,
            interactive: true,
        }
    }

    /// Tooltip preset: a chrome-less, **click-through** overlay. No scrim,
    /// no dismiss paths, and [`interactive`](Self::interactive) `false` so
    /// it never captures input — pointer events keep flowing to the trigger
    /// underneath, which is what lets a hover-driven tip dismiss itself when
    /// the cursor leaves (the caller pops it from its `on_hover_exit`).
    ///
    /// Pair with [`LayerAnchor::AnchorRect`] using the trigger's own layout
    /// rect (see [`Container::on_hover_enter`](crate::Container::on_hover_enter)).
    pub fn tooltip() -> Self {
        Self {
            anchor: LayerAnchor::ViewportCenter,
            scrim: None,
            dismiss_on_outside_click: false,
            dismiss_on_escape: false,
            block_shortcuts: false,
            interactive: false,
        }
    }

    /// Override the anchor.
    pub fn anchor(mut self, anchor: LayerAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Replace the scrim. Pass `None` to disable.
    pub fn scrim(mut self, color: Option<Color>) -> Self {
        self.scrim = color;
        self
    }

    /// Toggle the outside-click dismiss path.
    pub fn dismiss_on_outside_click(mut self, on: bool) -> Self {
        self.dismiss_on_outside_click = on;
        self
    }

    /// Toggle the Escape dismiss path.
    pub fn dismiss_on_escape(mut self, on: bool) -> Self {
        self.dismiss_on_escape = on;
        self
    }

    /// Toggle suppression of app-level shortcuts while this layer is on
    /// top. See [`Self::block_shortcuts`].
    pub fn block_shortcuts(mut self, on: bool) -> Self {
        self.block_shortcuts = on;
        self
    }

    /// Toggle whether this layer participates in event routing. Pass
    /// `false` for a click-through, paint-only overlay (tooltips). See
    /// [`Self::interactive`].
    pub fn interactive(mut self, on: bool) -> Self {
        self.interactive = on;
        self
    }
}

impl Default for LayerOptions {
    fn default() -> Self {
        Self::popover()
    }
}

/// Internal book-keeping for an active layer.
///
/// `offset` and `measured_size` are filled in by the layout pass and read by
/// paint / event dispatch. They go stale between layout passes, but the
/// event loop runs layout immediately before paint and right before
/// dispatching the next event batch, so the lag is invisible to widgets.
pub(crate) struct LayerEntry {
    pub root: usize,
    pub options: LayerOptions,
    pub offset: (f32, f32),
    pub measured_size: shroud_core::Size,
}
