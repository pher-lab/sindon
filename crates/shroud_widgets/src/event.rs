//! Event types and dispatch context.

use crate::layer::LayerOptions;
use crate::tree::WidgetTree;
use crate::widget::Widget;
use shroud_core::Point;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A named key.
    Named(NamedKey),
    /// A character key.
    Character(char),
}

/// Named keyboard keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The "Super" / "Meta" / "Windows" / "Command" key.
    pub logo: bool,
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
}

impl EventContext {
    pub fn new() -> Self {
        Self {
            modifiers: Modifiers::default(),
            commands: Vec::new(),
        }
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
    pub fn push_layer<F>(&mut self, options: LayerOptions, root_widget: impl Widget + 'static, populate: F)
    where
        F: FnOnce(&mut WidgetTree, usize) + 'static,
    {
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

    /// Drain queued commands. Intended for `WidgetTree` to call after a
    /// dispatch walk completes.
    pub(crate) fn take_commands(&mut self) -> Vec<TreeCommand> {
        std::mem::take(&mut self.commands)
    }
}

impl Default for EventContext {
    fn default() -> Self {
        Self::new()
    }
}
