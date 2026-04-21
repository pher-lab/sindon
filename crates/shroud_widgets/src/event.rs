//! Event types and dispatch context.

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
    /// A `MouseDown` landed *outside* this widget's bounds — sent to every
    /// widget the click did not hit, so focusable widgets (`Input`,
    /// `SecureInput`) can drop their focus on a click-outside. The tree
    /// broadcasts this after the normal `MouseDown` dispatch; handlers that
    /// do not track focus can ignore it.
    FocusLost,
    /// Keyboard key pressed.
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
}

/// Context passed to widgets during event handling.
///
/// Besides tracking focus, this is the sole route by which event handlers
/// request tree mutations (add/remove/replace-root/replace-screen). Commands
/// are queued and applied after the current dispatch finishes — an enqueued
/// remove does not invalidate indices for handlers that run later in the
/// same dispatch, which keeps traversal stable.
pub struct EventContext {
    /// The widget that currently has focus, if any.
    pub focused: Option<usize>,
    /// Deferred tree mutations queued during this dispatch. Drained by
    /// `WidgetTree::dispatch_event` once the walk completes.
    pub(crate) commands: Vec<TreeCommand>,
}

impl EventContext {
    pub fn new() -> Self {
        Self {
            focused: None,
            commands: Vec::new(),
        }
    }

    /// Request focus for a widget (by index in the tree).
    pub fn request_focus(&mut self, index: usize) {
        self.focused = Some(index);
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
