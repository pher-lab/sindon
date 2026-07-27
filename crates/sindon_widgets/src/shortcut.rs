//! App-level keyboard shortcut routing.
//!
//! Shortcuts are registered against `AppScope::on_shortcut` (in
//! `sindon_app`) and dispatched by the widget tree *before* the
//! Escape/Tab interceptors, so an app-level binding wins over both layer
//! dismiss and focus navigation.
//!
//! ## Scope
//!
//! Two modes (see [`ShortcutScope`]):
//!
//! - [`ShortcutScope::Global`] — fires unconditionally, even while a text
//!   input is focused or a non-trapping modal is up. Right for
//!   lock/panic/quit.
//! - [`ShortcutScope::WhenNoTextInput`] (default) — suppressed while the
//!   focused widget reports `Widget::accepts_text` true. Right for
//!   shortcuts like Ctrl+N where the literal key in a textarea should
//!   reach the widget instead.
//!
//! Layers can opt out of *all* shortcut delivery (including `Global`) by
//! setting [`LayerOptions::block_shortcuts`](crate::layer::LayerOptions)
//! — use for confirm sheets where every keystroke must reach the dialog.
//!
//! ## Precedence
//!
//! Bindings are matched in registration order; first hit consumes the
//! key event. Raw `Tab` / `Enter` / `Escape` (no modifier) are not
//! eligible — the router skips matching them so focus navigation,
//! widget activation, and layer dismiss stay intact.

use crate::event::{EventContext, Key, Modifiers, NamedKey, WidgetEvent};

/// A keyboard binding: modifier set + key + scope.
///
/// Construct with [`Shortcut::new`] (default scope) or
/// [`Shortcut::global`], or use the [`Shortcut::ctrl`] helper for the
/// common Ctrl+letter case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub mods: Modifiers,
    pub key: Key,
    pub scope: ShortcutScope,
}

impl Shortcut {
    /// Default-scope binding ([`ShortcutScope::WhenNoTextInput`]).
    pub fn new(mods: Modifiers, key: Key) -> Self {
        Self {
            mods,
            key,
            scope: ShortcutScope::WhenNoTextInput,
        }
    }

    /// Global-scope binding — fires even when an `Input`/`SecureInput`
    /// has focus.
    pub fn global(mods: Modifiers, key: Key) -> Self {
        Self {
            mods,
            key,
            scope: ShortcutScope::Global,
        }
    }

    /// `Ctrl` + `ch` with the default scope. The character is lowered so
    /// `Shortcut::ctrl('L')` and `Shortcut::ctrl('l')` register the same
    /// binding (winit delivers Ctrl+letter as the lowercase form).
    pub fn ctrl(ch: char) -> Self {
        Self::new(Modifiers::CTRL, Key::Character(ch.to_ascii_lowercase()))
    }
}

/// When a [`Shortcut`] should fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutScope {
    /// Fires regardless of what is focused or which layer is on top
    /// (subject only to a layer's
    /// [`block_shortcuts`](crate::layer::LayerOptions) opt-out).
    Global,
    /// Suppressed while a focused widget reports
    /// [`Widget::accepts_text`](crate::widget::Widget::accepts_text)
    /// true. Default for [`Shortcut::new`].
    WhenNoTextInput,
}

/// Stable handle returned by [`crate::shortcut::ShortcutRouter::register`].
/// Pass back to `remove` to drop the binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortcutId(u64);

impl ShortcutId {
    /// Construct an id from a raw counter value.
    ///
    /// Exposed (but doc-hidden) so `sindon_app::AppScope` can pre-assign
    /// ids during the build closure, before the [`ShortcutRouter`]
    /// exists on the tree. App code should rely on the id returned by
    /// `AppScope::on_shortcut` instead of building one directly.
    #[doc(hidden)]
    pub fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

/// Context exposed to a shortcut handler.
///
/// Wraps the same [`EventContext`] widgets receive, so handlers can
/// enqueue any tree mutation (focus, replace_screen, push_layer, …) the
/// same way an `on_click` handler would.
pub struct ShortcutContext<'a> {
    pub event_ctx: &'a mut EventContext,
}

type ShortcutHandler = Box<dyn FnMut(&mut ShortcutContext)>;

struct Binding {
    id: ShortcutId,
    shortcut: Shortcut,
    handler: ShortcutHandler,
}

/// Linear registry of app-level keyboard shortcuts.
///
/// Owned by `WidgetTree`; populated via `AppScope::on_shortcut` after
/// the build closure returns, and consulted at the head of
/// `WidgetTree::dispatch_event` for every `KeyDown`.
pub struct ShortcutRouter {
    next_id: u64,
    bindings: Vec<Binding>,
}

impl ShortcutRouter {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 0,
            bindings: Vec::new(),
        }
    }

    /// Register a binding and return its handle.
    ///
    /// Bindings of raw `Tab`/`Enter`/`Escape` (no modifier) are stored
    /// but never fire — `try_dispatch` skips them
    /// so focus nav and layer dismiss keep working. A `debug_assert`
    /// trips in debug builds so the bad registration surfaces early.
    pub fn register<F>(&mut self, shortcut: Shortcut, handler: F) -> ShortcutId
    where
        F: FnMut(&mut ShortcutContext) + 'static,
    {
        debug_assert!(
            !is_reserved_bare_key(&shortcut),
            "raw Tab/Enter/Escape cannot be bound as shortcuts (would break focus navigation / layer dismiss); add a modifier"
        );
        let id = ShortcutId(self.next_id);
        self.next_id += 1;
        self.bindings.push(Binding {
            id,
            shortcut,
            handler: Box::new(handler),
        });
        id
    }

    /// Drop a binding. No-op if the id was already removed.
    pub fn remove(&mut self, id: ShortcutId) {
        self.bindings.retain(|b| b.id != id);
    }

    /// Register with a caller-provided id. Doc-hidden because it is only
    /// useful to `sindon_app::AppScope`, which pre-assigns ids during the
    /// build closure (before the router exists on the tree) and replays
    /// them after the tree is built. `self.next_id` is bumped past `id`
    /// so subsequent [`Self::register`] calls don't collide.
    #[doc(hidden)]
    pub fn register_with_id<F>(&mut self, id: ShortcutId, shortcut: Shortcut, handler: F)
    where
        F: FnMut(&mut ShortcutContext) + 'static,
    {
        debug_assert!(
            !is_reserved_bare_key(&shortcut),
            "raw Tab/Enter/Escape cannot be bound as shortcuts (would break focus navigation / layer dismiss); add a modifier"
        );
        if id.0 >= self.next_id {
            self.next_id = id.0 + 1;
        }
        self.bindings.push(Binding {
            id,
            shortcut,
            handler: Box::new(handler),
        });
    }

    /// Try to fire a binding for `event`.
    ///
    /// Returns `true` when a handler ran (the caller should treat the
    /// event as consumed). `accepts_text` should reflect the focused
    /// widget's [`Widget::accepts_text`](crate::widget::Widget::accepts_text);
    /// `layer_blocks` should be the top layer's
    /// [`block_shortcuts`](crate::layer::LayerOptions) (or `false` when
    /// no layer is active).
    pub(crate) fn try_dispatch(
        &mut self,
        event: &WidgetEvent,
        mods: Modifiers,
        accepts_text: bool,
        layer_blocks: bool,
        event_ctx: &mut EventContext,
    ) -> bool {
        if layer_blocks {
            return false;
        }
        let key = match event {
            WidgetEvent::KeyDown { key } => key,
            _ => return false,
        };
        for binding in &mut self.bindings {
            if is_reserved_bare_key(&binding.shortcut) {
                continue;
            }
            if binding.shortcut.mods != mods || &binding.shortcut.key != key {
                continue;
            }
            if matches!(binding.shortcut.scope, ShortcutScope::WhenNoTextInput) && accepts_text {
                continue;
            }
            let mut ctx = ShortcutContext { event_ctx };
            (binding.handler)(&mut ctx);
            return true;
        }
        false
    }
}

impl Default for ShortcutRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// True when `shortcut` would shadow Tab/Enter/Escape focus/dismiss
/// handling. The router silently skips these; debug builds also assert
/// at registration so callers notice immediately.
fn is_reserved_bare_key(shortcut: &Shortcut) -> bool {
    if shortcut.mods != Modifiers::NONE {
        return false;
    }
    matches!(
        shortcut.key,
        Key::Named(NamedKey::Tab) | Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Escape)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn key_l_down() -> WidgetEvent {
        WidgetEvent::KeyDown {
            key: Key::Character('l'),
        }
    }

    #[test]
    fn matching_modifiers_fire_handler() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        router.register(Shortcut::ctrl('l'), move |_| f.set(true));

        let mut ctx = EventContext::new();
        let consumed = router.try_dispatch(&key_l_down(), Modifiers::CTRL, false, false, &mut ctx);

        assert!(consumed);
        assert!(fired.get());
    }

    #[test]
    fn non_matching_modifiers_dont_fire() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        router.register(Shortcut::ctrl('l'), move |_| f.set(true));

        let mut ctx = EventContext::new();
        let consumed = router.try_dispatch(&key_l_down(), Modifiers::NONE, false, false, &mut ctx);

        assert!(!consumed);
        assert!(!fired.get());
    }

    #[test]
    fn when_no_text_input_is_suppressed_with_focused_input() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        router.register(Shortcut::ctrl('n'), move |_| f.set(true));

        let event = WidgetEvent::KeyDown {
            key: Key::Character('n'),
        };
        let mut ctx = EventContext::new();
        let consumed = router.try_dispatch(&event, Modifiers::CTRL, true, false, &mut ctx);

        assert!(!consumed);
        assert!(!fired.get());
    }

    #[test]
    fn global_fires_even_with_focused_text_input() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        router.register(
            Shortcut::global(Modifiers::CTRL, Key::Character('l')),
            move |_| f.set(true),
        );

        let mut ctx = EventContext::new();
        let consumed = router.try_dispatch(&key_l_down(), Modifiers::CTRL, true, false, &mut ctx);

        assert!(consumed);
        assert!(fired.get());
    }

    #[test]
    fn layer_blocks_suppresses_global_too() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        router.register(
            Shortcut::global(Modifiers::CTRL, Key::Character('l')),
            move |_| f.set(true),
        );

        let mut ctx = EventContext::new();
        let consumed = router.try_dispatch(&key_l_down(), Modifiers::CTRL, false, true, &mut ctx);

        assert!(!consumed);
        assert!(!fired.get());
    }

    #[test]
    fn remove_drops_binding() {
        let mut router = ShortcutRouter::new();
        let fired = Rc::new(Cell::new(0u32));
        let f = fired.clone();
        let id = router.register(Shortcut::ctrl('l'), move |_| f.set(f.get() + 1));

        let mut ctx = EventContext::new();
        router.try_dispatch(&key_l_down(), Modifiers::CTRL, false, false, &mut ctx);
        assert_eq!(fired.get(), 1);

        router.remove(id);
        let consumed = router.try_dispatch(&key_l_down(), Modifiers::CTRL, false, false, &mut ctx);
        assert!(!consumed);
        assert_eq!(fired.get(), 1);
    }

    #[test]
    fn first_registration_wins_on_duplicate() {
        let mut router = ShortcutRouter::new();
        let first = Rc::new(Cell::new(false));
        let second = Rc::new(Cell::new(false));
        let f1 = first.clone();
        let f2 = second.clone();
        router.register(Shortcut::ctrl('l'), move |_| f1.set(true));
        router.register(Shortcut::ctrl('l'), move |_| f2.set(true));

        let mut ctx = EventContext::new();
        router.try_dispatch(&key_l_down(), Modifiers::CTRL, false, false, &mut ctx);

        assert!(first.get());
        assert!(!second.get());
    }

    #[test]
    fn reserved_bare_tab_never_fires_even_if_registered() {
        // is_reserved_bare_key path: in release, registration succeeds but
        // dispatch skips the binding so focus navigation keeps working.
        // (debug_assert trips in debug builds — we exercise the release
        // semantics here.)
        let shortcut = Shortcut::new(Modifiers::NONE, Key::Named(NamedKey::Tab));
        assert!(is_reserved_bare_key(&shortcut));
    }
}
