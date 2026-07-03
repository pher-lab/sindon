//! Event types and dispatch context.

use crate::layer::{LayerAnchor, LayerOptions};
use crate::tree::WidgetTree;
use crate::widget::Widget;
use shroud_core::{Point, Rect};

/// Events that widgets can receive.
#[derive(Debug, Clone)]
pub enum WidgetEvent {
    /// Mouse button pressed at the given position.
    MouseDown {
        position: Point,
        button: MouseButton,
    },
    /// Mouse button released at the given position.
    MouseUp {
        position: Point,
        button: MouseButton,
    },
    /// Mouse moved to the given position.
    MouseMove { position: Point },
    /// Mouse entered this widget's bounds.
    MouseEnter,
    /// Mouse left this widget's bounds.
    MouseLeave,
    /// The widget has lost focus.
    ///
    /// Dispatched to the widget that was the
    /// [`FocusManager`](crate::focus::FocusManager)'s `focused` target
    /// just before a focus change. Sources of focus change:
    /// - **Tab / Shift+Tab**: tree's keyboard focus routing.
    /// - **MouseDown**: click on a focusable widget (new target), or
    ///   click on a non-focusable region (clears focus to `None`).
    /// - **Programmatic**: `WidgetTree::focus(...)` from app code.
    ///
    /// Widgets that maintain a self-managed focus flag should flip it to
    /// `false` here. Handlers that do not track focus can ignore it.
    FocusLost,
    /// The widget has gained focus.
    ///
    /// Dispatched to the widget that just became the
    /// [`FocusManager`](crate::focus::FocusManager)'s `focused` target,
    /// paired with a `FocusLost` to the previous one if any. Same sources
    /// as [`FocusLost`](Self::FocusLost): Tab nav, click on a focusable
    /// widget, and programmatic `WidgetTree::focus`. Widgets that
    /// maintain a self-managed focus flag should flip it to `true` here.
    FocusGained,
    /// Keyboard key pressed. Modifier state at the time of the press is
    /// available via [`EventContext::modifiers`].
    KeyDown { key: Key },
    /// Keyboard key released.
    KeyUp { key: Key },
    /// Character input (after IME processing).
    CharInput { ch: char },
    /// IME preedit (composition) update: the in-progress, *uncommitted* text
    /// the user is composing via an IME (Japanese / Chinese / Korean, dead
    /// keys, …), plus an optional caret byte range within that text.
    ///
    /// winit delivers this while composing, and once the IME is app-driven
    /// (`set_ime_allowed(true)`) the OS no longer draws an inline composition
    /// string — the application must render the preedit itself. The committed
    /// result arrives separately as a burst of [`CharInput`](Self::CharInput).
    /// An empty `text` clears the preedit (composition cancelled or just
    /// committed). The focused text widget renders it inline at the caret;
    /// widgets that don't accept text ignore it.
    ImePreedit {
        text: String,
        /// Caret byte range `(start, end)` within `text`, or `None` to hide
        /// the caret during composition (winit's own convention).
        cursor: Option<(usize, usize)>,
    },
    /// Scroll wheel (or trackpad scroll).
    ///
    /// `position` is the cursor location at the time of the scroll, used to
    /// route the event to the correct scrolling container.
    Scroll {
        position: Point,
        delta_x: f32,
        delta_y: f32,
    },
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard key (simplified).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A named key.
    Named(NamedKey),
    /// A character key.
    Character(char),
}

/// Named keyboard keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
}

/// Result of handling an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was consumed — stop propagation.
    Consumed,
    /// Event was not handled — continue propagation.
    Ignored,
}

/// Builder closure for `TreeCommand::RebuildChildren`. Factored out to
/// appease `clippy::type_complexity`.
type RebuildBuilder = Box<dyn FnOnce(&mut WidgetTree, usize)>;

/// Builder closure for `TreeCommand::PushLayer`. Receives the tree plus
/// the freshly-pushed layer's root index so children can be added under
/// it via `tree.add_child(root, ...)`.
type LayerPopulator = Box<dyn FnOnce(&mut WidgetTree, usize)>;

/// Deferred tree mutation requested by an event handler.
///
/// Event handlers run while `WidgetTree` is mid-walk, so they can't borrow
/// the tree mutably. Instead they enqueue a `TreeCommand` via
/// [`EventContext`], and the tree drains the queue once dispatch returns.
pub(crate) enum TreeCommand {
    AddChild {
        parent: usize,
        widget: Box<dyn Widget + 'static>,
    },
    Remove {
        idx: usize,
    },
    ReplaceRoot {
        widget: Box<dyn Widget + 'static>,
    },
    /// Whole-tree rebuild. The closure receives an empty tree (old root
    /// already tombstoned) and populates it from scratch. Used for screen
    /// transitions where both the root and its descendants change.
    ReplaceScreen {
        build: Box<dyn FnOnce(&mut WidgetTree)>,
    },
    /// Subtree rebuild. The closure runs after every current child of
    /// `parent` has been tombstoned; it receives the tree and the stable
    /// parent index so it can `tree.add_child(parent, ...)` freely.
    RebuildChildren {
        parent: usize,
        build: RebuildBuilder,
    },
    /// Programmatic focus change. `Some(idx)` focuses that widget,
    /// `None` clears focus. Dispatched through `WidgetTree::focus`, so
    /// `FocusLost`/`FocusGained` fire on the affected widgets and any
    /// commands those handlers enqueue are drained on the next pass.
    Focus {
        target: Option<usize>,
    },
    /// Push a new overlay layer. `root_widget` becomes the layer's root,
    /// then `populate` runs to add children. Mirrors `ReplaceScreen`'s
    /// "root + populator" shape so the dispatch site never needs the
    /// layer's index synchronously.
    PushLayer {
        options: LayerOptions,
        root_widget: Box<dyn Widget + 'static>,
        populate: LayerPopulator,
    },
    /// Pop a specific layer by its root index. No-op when the layer is
    /// already gone (e.g. dismissed by an outside-click before the
    /// command drained).
    PopLayer {
        root: usize,
    },
    /// Pop whichever layer is currently on top. No-op when no layer is
    /// active.
    PopTopLayer,
}

/// Keyboard modifier state at the time of an event.
///
/// Populated from winit's `ModifiersChanged` by the event loop and
/// exposed on [`EventContext::modifiers`]. Widgets read the relevant
/// flag inside their `event` handler — e.g. `ctx.modifiers.shift` to
/// distinguish Tab from Shift+Tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The "Super" / "Meta" / "Windows" / "Command" key.
    pub logo: bool,
}

impl Modifiers {
    /// No modifiers pressed. Same as [`Modifiers::default`] but available in
    /// `const` contexts (useful for `Shortcut` constants).
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        logo: false,
    };

    /// Just `Ctrl` held.
    pub const CTRL: Self = Self {
        shift: false,
        ctrl: true,
        alt: false,
        logo: false,
    };

    /// Just `Shift` held.
    pub const SHIFT: Self = Self {
        shift: true,
        ctrl: false,
        alt: false,
        logo: false,
    };

    /// Just `Alt` held.
    pub const ALT: Self = Self {
        shift: false,
        ctrl: false,
        alt: true,
        logo: false,
    };

    /// Just the logo / super / cmd key held.
    pub const LOGO: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        logo: true,
    };

    /// Union of two modifier sets. Pure (`const`) so combinations like
    /// `Modifiers::CTRL.or(Modifiers::SHIFT)` work in `const` contexts.
    pub const fn or(self, other: Self) -> Self {
        Self {
            shift: self.shift || other.shift,
            ctrl: self.ctrl || other.ctrl,
            alt: self.alt || other.alt,
            logo: self.logo || other.logo,
        }
    }

    /// True when any non-shift modifier is held (`ctrl`, `alt`, or `logo`).
    ///
    /// Used by the event loop to decide whether a winit `Character` event
    /// should be promoted to a `KeyDown` (so the shortcut router can see
    /// e.g. Ctrl+L) instead of arriving as `CharInput` (which `Input`
    /// would consume as a literal 'l').
    pub const fn has_non_shift(self) -> bool {
        self.ctrl || self.alt || self.logo
    }
}

/// Context passed to widgets during event handling.
///
/// Carries modifier state (populated by the event loop from winit's
/// `ModifiersChanged`) and the deferred command queue used by handlers
/// to request tree mutations (add/remove/replace-root/replace-screen).
/// Commands are applied after the current dispatch finishes — an
/// enqueued remove does not invalidate indices for handlers that run
/// later in the same dispatch, which keeps traversal stable.
pub struct EventContext {
    /// Current keyboard modifier state. Updated by the event loop on
    /// `ModifiersChanged`; read by widgets (and by the tree's Tab
    /// routing) during event handling.
    pub modifiers: Modifiers,
    /// Deferred tree mutations queued during this dispatch. Drained by
    /// `WidgetTree::dispatch_event` once the walk completes.
    pub(crate) commands: Vec<TreeCommand>,
    /// Text a handler asked to place on the system clipboard during this
    /// dispatch (e.g. a copy / cut from a focused `Input`). The event loop
    /// drains it after dispatch and writes it to the OS clipboard — widgets
    /// have no direct clipboard access. `None` when nothing was copied.
    pub(crate) clipboard_write: Option<String>,
    /// Pointer-capture request a handler made this dispatch:
    /// `Some(true)` = capture the pointer for the widget that is currently
    /// handling the event, `Some(false)` = release it, `None` = no change.
    /// The tree binds an acquire to the dispatched-to widget and routes
    /// subsequent `MouseMove` / `MouseUp` straight to it. See
    /// [`Self::capture_pointer`].
    pub(crate) capture_change: Option<bool>,
    /// Viewport offset of the layer whose subtree is currently dispatching
    /// — `(0, 0)` for the main tree. Set by
    /// [`WidgetTree::dispatch_event`](crate::tree::WidgetTree::dispatch_event)
    /// from the active layer's anchor offset.
    ///
    /// Widgets inside a layer see *layer-local* rects in their `event`
    /// (and a layer-local cursor position), matching paint. So a handler
    /// that anchors a child popover to its own rect would, without
    /// correction, place it relative to the layer's origin instead of the
    /// viewport (the dropdown-in-a-popover bug). [`Self::push_layer`]
    /// translates an [`LayerAnchor::AnchorRect`] by this offset so the rect
    /// the handler passes — in its own local space — lands at the right
    /// viewport position. Exposed via [`Self::layer_offset`] for widgets
    /// that need the translation explicitly.
    pub(crate) current_layer_offset: (f32, f32),
}

impl EventContext {
    pub fn new() -> Self {
        Self {
            modifiers: Modifiers::default(),
            commands: Vec::new(),
            clipboard_write: None,
            capture_change: None,
            current_layer_offset: (0.0, 0.0),
        }
    }

    /// Viewport offset of the layer currently dispatching this event, or
    /// `(0, 0)` when the main tree is handling it. Add this to a layer-local
    /// rect (e.g. a widget's `event` `layout`) to get viewport coordinates.
    ///
    /// [`Self::push_layer`] applies this automatically to an
    /// [`LayerAnchor::AnchorRect`], so most callers don't need it directly;
    /// it's exposed for widgets that compute an anchor some other way.
    pub fn layer_offset(&self) -> (f32, f32) {
        self.current_layer_offset
    }

    /// Queue a widget to be inserted as a child of `parent` after the
    /// current dispatch finishes.
    ///
    /// Applied in the order commands are enqueued. If `parent` has been
    /// tombstoned by a prior command (or is otherwise invalid) the request
    /// is silently dropped — stable behavior when multiple handlers race.
    pub fn add_child(&mut self, parent: usize, widget: impl Widget + 'static) {
        self.commands.push(TreeCommand::AddChild {
            parent,
            widget: Box::new(widget),
        });
    }

    /// Queue a widget (and its subtree) for removal after the current
    /// dispatch finishes.
    ///
    /// The widget's index is tombstoned: existing indices for surviving
    /// widgets stay stable, and the removed index does not get reassigned.
    /// Applied best-effort — a second remove of the same index is a no-op.
    pub fn remove(&mut self, idx: usize) {
        self.commands.push(TreeCommand::Remove { idx });
    }

    /// Queue a whole-tree swap: remove the current root (if any) and
    /// install `widget` as the new, childless root.
    ///
    /// For screens composed of multiple widgets, use
    /// [`Self::replace_screen`] instead.
    pub fn replace_root(&mut self, widget: impl Widget + 'static) {
        self.commands.push(TreeCommand::ReplaceRoot {
            widget: Box::new(widget),
        });
    }

    /// Queue a full screen transition. The closure runs against an empty
    /// tree (the previous root and its descendants have already been
    /// tombstoned) and rebuilds the hierarchy — typically via
    /// `tree.set_root(...)` followed by `tree.add_child(...)` calls.
    ///
    /// Dropping the old subtree runs every widget's `Drop` impl, so secure
    /// widgets (`SecureInput`, `SecureText`) zeroize their backing memory
    /// as part of the transition.
    pub fn replace_screen<F>(&mut self, build: F)
    where
        F: FnOnce(&mut WidgetTree) + 'static,
    {
        self.commands.push(TreeCommand::ReplaceScreen {
            build: Box::new(build),
        });
    }

    /// Queue a surgical rebuild of `parent`'s children. The parent itself
    /// stays in place (keeping its index stable for other captured closures);
    /// every current descendant is tombstoned, and then `build` runs to
    /// populate fresh children via `tree.add_child(parent, ...)`.
    ///
    /// Scoped alternative to [`Self::replace_screen`] for dynamic lists: use
    /// when only one section of the UI changes (e.g. a password manager's
    /// entry list grows by one row) so the surrounding widgets — inputs the
    /// user is typing into, scroll offsets on sibling containers — aren't
    /// disturbed.
    ///
    /// Tombstoned children drop in post-order, so secure widgets
    /// (`SecureInput`, `SecureText`) zeroize as part of the rebuild. Silently
    /// drops the command if `parent` has been tombstoned by a prior queued
    /// command.
    pub fn rebuild_children<F>(&mut self, parent: usize, build: F)
    where
        F: FnOnce(&mut WidgetTree, usize) + 'static,
    {
        self.commands.push(TreeCommand::RebuildChildren {
            parent,
            build: Box::new(build),
        });
    }

    /// Queue a focus change to land after the current dispatch finishes.
    ///
    /// `idx` becomes the new focused widget. The previously focused widget
    /// (if any) receives `FocusLost`; `idx` receives `FocusGained` — same
    /// path as Tab routing and click-to-focus.
    ///
    /// Silently dropped if `idx` is tombstoned by the time commands drain.
    /// Use [`Self::blur`] to clear focus instead.
    pub fn focus(&mut self, idx: usize) {
        self.commands.push(TreeCommand::Focus { target: Some(idx) });
    }

    /// Queue a focus clear. The currently focused widget (if any) receives
    /// `FocusLost`; no widget gains focus afterwards.
    pub fn blur(&mut self) {
        self.commands.push(TreeCommand::Focus { target: None });
    }

    /// Open a new overlay layer. `root_widget` provides the layer's root
    /// container, then `populate` runs against the freshly-installed
    /// layer to add children — same shape as
    /// [`Self::replace_screen`] but scoped to a single layer instead of
    /// the whole tree.
    ///
    /// Layers paint over the main tree in push order (last pushed =
    /// topmost) and capture all pointer / keyboard input while active.
    /// See [`LayerOptions::modal`] / [`LayerOptions::popover`] for the
    /// common configurations.
    ///
    /// The command is enqueued and runs after the current dispatch
    /// completes; the topmost layer is the one most recently pushed when
    /// the drain settles.
    ///
    /// When the handler pushing this layer is itself inside a layer, an
    /// [`LayerAnchor::AnchorRect`] is translated from that layer's local
    /// space into viewport coordinates by [`Self::layer_offset`]. This is
    /// what lets a [`Dropdown`](crate::Dropdown) (or any anchored popover)
    /// open in the right place when it lives inside a modal or another
    /// popover — its `event` `layout` rect is layer-local, and this
    /// correction puts the child layer under the trigger rather than at
    /// the parent layer's origin. A no-op for the main tree (offset zero).
    /// Only [`LayerAnchor::AnchorRect`] is translated; [`ViewportCenter`] and
    /// [`Viewport`] are viewport-absolute and always left untouched.
    ///
    /// [`ViewportCenter`]: LayerAnchor::ViewportCenter
    /// [`Viewport`]: LayerAnchor::Viewport
    pub fn push_layer<F>(
        &mut self,
        mut options: LayerOptions,
        root_widget: impl Widget + 'static,
        populate: F,
    ) where
        F: FnOnce(&mut WidgetTree, usize) + 'static,
    {
        if let LayerAnchor::AnchorRect {
            rect,
            prefer,
            align,
        } = options.anchor
        {
            let (ox, oy) = self.current_layer_offset;
            if ox != 0.0 || oy != 0.0 {
                options.anchor = LayerAnchor::AnchorRect {
                    rect: Rect::new(
                        rect.origin.x + ox,
                        rect.origin.y + oy,
                        rect.size.width,
                        rect.size.height,
                    ),
                    prefer,
                    align,
                };
            }
        }
        self.commands.push(TreeCommand::PushLayer {
            options,
            root_widget: Box::new(root_widget),
            populate: Box::new(populate),
        });
    }

    /// Dismiss the layer whose root is `root`. No-op when that layer is
    /// already gone (e.g. dismissed by an outside-click before this
    /// command drained).
    pub fn pop_layer(&mut self, root: usize) {
        self.commands.push(TreeCommand::PopLayer { root });
    }

    /// Dismiss whichever layer is currently on top. No-op when no layer
    /// is active by the time the command drains.
    pub fn pop_top_layer(&mut self) {
        self.commands.push(TreeCommand::PopTopLayer);
    }

    /// Request that `text` be written to the system clipboard once the
    /// current dispatch finishes.
    ///
    /// Widgets can't touch the OS clipboard directly (it lives in the
    /// platform layer the event loop owns), so a copy / cut handler stashes
    /// the text here and the event loop flushes it. The most recent call in a
    /// dispatch wins. Used by [`Input`](crate::Input)'s Ctrl+C / Ctrl+X.
    pub fn write_clipboard(&mut self, text: impl Into<String>) {
        self.clipboard_write = Some(text.into());
    }

    /// Capture the pointer for the widget currently handling the event.
    ///
    /// While captured, the tree routes every `MouseMove` and `MouseUp`
    /// straight to this widget — bypassing hit-testing — so a drag keeps
    /// being delivered even when the cursor leaves the widget's rect. Call
    /// from a `MouseDown` handler that begins a drag (e.g. `Input`'s
    /// drag-select); pair with [`Self::release_pointer`] on `MouseUp`.
    /// Capture is also dropped automatically if the widget is removed.
    pub fn capture_pointer(&mut self) {
        self.capture_change = Some(true);
    }

    /// Release a pointer capture taken via [`Self::capture_pointer`].
    ///
    /// No-op unless this widget currently holds the capture. Call from the
    /// `MouseUp` handler that ends the drag.
    pub fn release_pointer(&mut self) {
        self.capture_change = Some(false);
    }

    /// Take any pending pointer-capture change requested this dispatch.
    /// Intended for `WidgetTree` to call right after a widget's `event`
    /// returns, so an acquire binds to that widget. Leaves `None` behind.
    pub(crate) fn take_capture_change(&mut self) -> Option<bool> {
        self.capture_change.take()
    }

    /// Drain queued commands. Intended for `WidgetTree` to call after a
    /// dispatch walk completes.
    pub(crate) fn take_commands(&mut self) -> Vec<TreeCommand> {
        std::mem::take(&mut self.commands)
    }

    /// Take any pending clipboard write requested via [`Self::write_clipboard`].
    /// Intended for the event loop to call after dispatch so it can hand the
    /// text to the platform clipboard. Leaves `None` behind.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.clipboard_write.take()
    }
}

impl Default for EventContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Container;
    use crate::layer::{HAlign, Placement, VAlign};

    fn pushed_anchor(ctx: &EventContext) -> LayerAnchor {
        match ctx.commands.last().expect("a command was queued") {
            TreeCommand::PushLayer { options, .. } => options.anchor,
            _ => panic!("expected a PushLayer command"),
        }
    }

    #[test]
    fn push_layer_translates_anchor_by_active_layer_offset() {
        // A handler inside a layer at viewport offset (100, 50) anchors a
        // popover to its own *layer-local* rect; push_layer must translate it
        // into viewport space so the child lands under the trigger rather than
        // at the parent layer's origin (gap G14).
        let mut ctx = EventContext::new();
        ctx.current_layer_offset = (100.0, 50.0);
        ctx.push_layer(
            LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
                rect: Rect::new(8.0, 40.0, 120.0, 24.0),
                prefer: Placement::Below,
                align: HAlign::Start,
            }),
            Container::column(),
            |_tree, _root| {},
        );
        match pushed_anchor(&ctx) {
            LayerAnchor::AnchorRect { rect, prefer, .. } => {
                assert_eq!((rect.origin.x, rect.origin.y), (108.0, 90.0));
                assert_eq!((rect.size.width, rect.size.height), (120.0, 24.0));
                assert_eq!(prefer, Placement::Below);
            }
            other => panic!("expected AnchorRect, got {other:?}"),
        }
    }

    #[test]
    fn push_layer_leaves_anchor_unchanged_in_main_tree() {
        // Offset zero (main tree): the rect a handler passes is already in
        // viewport space and must be left exactly as-is.
        let mut ctx = EventContext::new();
        ctx.push_layer(
            LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
                rect: Rect::new(8.0, 40.0, 0.0, 0.0),
                prefer: Placement::Below,
                align: HAlign::Start,
            }),
            Container::column(),
            |_tree, _root| {},
        );
        match pushed_anchor(&ctx) {
            LayerAnchor::AnchorRect { rect, .. } => {
                assert_eq!((rect.origin.x, rect.origin.y), (8.0, 40.0));
            }
            other => panic!("expected AnchorRect, got {other:?}"),
        }
    }

    #[test]
    fn push_layer_preserves_anchor_rect_alignment() {
        // The horizontal alignment must survive the layer-offset translation
        // so a right-aligned (CSS `right-0`) menu stays right-aligned when it
        // is nested inside another layer.
        let mut ctx = EventContext::new();
        ctx.current_layer_offset = (100.0, 50.0);
        ctx.push_layer(
            LayerOptions::popover().anchor(LayerAnchor::AnchorRect {
                rect: Rect::new(8.0, 40.0, 120.0, 24.0),
                prefer: Placement::Auto,
                align: HAlign::End,
            }),
            Container::column(),
            |_tree, _root| {},
        );
        match pushed_anchor(&ctx) {
            LayerAnchor::AnchorRect { align, .. } => assert_eq!(align, HAlign::End),
            other => panic!("expected AnchorRect, got {other:?}"),
        }
    }

    #[test]
    fn push_layer_viewport_center_is_never_shifted() {
        // Only AnchorRect is offset-relative; ViewportCenter (modals, the
        // error banner) must never be nudged by a stale layer offset.
        let mut ctx = EventContext::new();
        ctx.current_layer_offset = (100.0, 50.0);
        ctx.push_layer(
            LayerOptions::popover().anchor(LayerAnchor::ViewportCenter),
            Container::column(),
            |_tree, _root| {},
        );
        assert!(matches!(pushed_anchor(&ctx), LayerAnchor::ViewportCenter));
    }

    #[test]
    fn push_layer_viewport_anchor_is_never_shifted() {
        // A `Viewport` anchor is resolved against the viewport, not the
        // pushing layer, so a stale layer offset must never nudge it.
        let mut ctx = EventContext::new();
        ctx.current_layer_offset = (100.0, 50.0);
        ctx.push_layer(
            LayerOptions::popover().anchor(LayerAnchor::Viewport {
                h: HAlign::Center,
                v: VAlign::Start,
                offset: (0.0, 8.0),
            }),
            Container::column(),
            |_tree, _root| {},
        );
        match pushed_anchor(&ctx) {
            LayerAnchor::Viewport { h, v, offset } => {
                assert_eq!(h, HAlign::Center);
                assert_eq!(v, VAlign::Start);
                assert_eq!(offset, (0.0, 8.0));
            }
            other => panic!("expected Viewport, got {other:?}"),
        }
    }
}
