//! `ClearTrigger` — signal-backed handle that tells a bound widget to clear.
//!
//! Introduced in Phase 18d for `SecureInput`: the reactive graph shouldn't
//! carry a plain-text copy of a secret, so we deliberately *don't* give
//! `SecureInput` a `Reactive<SecureString>`. Instead, the app holds a
//! `ClearTrigger`, calls [`bump`](ClearTrigger::bump) when the widget should
//! wipe its buffer, and on its next paint/event the widget notices the
//! version change and zeroizes. This keeps the secret buffer sole-owned by
//! the widget.
//!
//! Shape: a wrapper around `Signal<u32>`. Each `bump` increments the
//! counter; the widget caches the last observed value in a `Cell<u32>` and
//! clears iff they differ.

use sindon_reactive::Signal;

/// Counter-backed trigger for clearing a bound widget (currently
/// [`SecureInput`](crate::SecureInput)). `Copy` — cheap to pass to handlers.
#[derive(Clone, Copy)]
pub struct ClearTrigger {
    version: Signal<u32>,
}

impl ClearTrigger {
    /// Create a new trigger starting at version 0.
    pub fn new() -> Self {
        Self {
            version: Signal::new(0u32),
        }
    }

    /// Increment the version. Any widget bound via `clear_on(trigger)` will
    /// observe the change on its next paint/event and clear itself.
    ///
    /// Wraps on overflow — only the *inequality* between widget and trigger
    /// matters, not the absolute count.
    pub fn bump(&self) {
        self.version.update(|v| *v = v.wrapping_add(1));
    }

    /// Current version (used by widgets to detect changes).
    pub(crate) fn version(&self) -> u32 {
        self.version.get()
    }
}

impl Default for ClearTrigger {
    fn default() -> Self {
        Self::new()
    }
}
