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
//! Two anchors are implemented:
//!
//! - [`LayerAnchor::ViewportCenter`] — modal dialogs.
//! - [`LayerAnchor::AnchorRect`] — popovers attached to a trigger rect
//!   (dropdowns, context menus). The variant is `#[non_exhaustive]` so
//!   absolute-positioned anchors can be added later without a break.

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

/// Where a layer is placed relative to the viewport or another widget.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum LayerAnchor {
    /// Center the layer's natural-size root inside the viewport. The
    /// standard modal-dialog placement.
    ViewportCenter,
    /// Anchor the layer to a trigger rect (screen coordinates). The
    /// layer's left edge aligns with `rect.x`; vertical placement follows
    /// `prefer`, with `Auto` flipping above when below would overflow.
    /// The x-coordinate is clamped so the popover stays inside the
    /// viewport.
    ///
    /// Typical use: a [`Dropdown`](crate::Dropdown) reads its own layout
    /// rect inside its click handler and pushes a popover layer with this
    /// anchor variant.
    AnchorRect { rect: Rect, prefer: Placement },
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
