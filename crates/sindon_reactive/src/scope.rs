use crate::node::ReactiveId;
use crate::runtime::RUNTIME;

/// An ownership boundary for reactive primitives.
///
/// All signals, memos, and effects created inside `scope.run(|| ...)` are owned
/// by this scope. When the scope is dropped (or explicitly disposed), all owned
/// nodes are removed from the reactive graph, cleanup callbacks run in reverse
/// order, and subscribers are unlinked.
///
/// Scopes can be nested — disposing a parent disposes all children first.
pub struct Scope {
    id: ReactiveId,
}

impl Scope {
    /// Create a new reactive scope.
    pub fn new() -> Self {
        let id = RUNTIME.with(|rt| rt.create_scope());
        Scope { id }
    }

    /// Execute a closure inside this scope.
    ///
    /// Any reactive primitives created during `f` are owned by this scope.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        RUNTIME.with(|rt| rt.run_in_scope(self.id, f))
    }

    /// Register a cleanup function that runs when this scope is disposed.
    ///
    /// Cleanups execute in reverse registration order (LIFO).
    pub fn on_cleanup(&self, f: impl FnOnce() + 'static) {
        RUNTIME.with(|rt| rt.add_cleanup(self.id, f));
    }

    /// Explicitly dispose this scope (equivalent to dropping it).
    pub fn dispose(self) {
        // Drop impl handles disposal
    }

    /// Return the reactive node id (for testing / debugging).
    pub fn id(&self) -> ReactiveId {
        self.id
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        // try_with avoids panic if thread-local is already destroyed
        let _ = RUNTIME.try_with(|rt| {
            rt.dispose_scope(self.id);
        });
    }
}

/// Register a cleanup function on the current scope (if any).
///
/// This is a convenience function — it does nothing if called outside a scope.
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    RUNTIME.with(|rt| {
        if let Some(scope_id) = rt.current_scope() {
            rt.add_cleanup(scope_id, f);
        }
    });
}
