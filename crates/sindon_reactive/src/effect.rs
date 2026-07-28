use crate::runtime::RUNTIME;

/// A reactive side-effect that automatically re-runs when its dependencies change.
///
/// Effects are executed immediately upon creation to capture initial dependencies.
/// They are owned by the enclosing `Scope` (if any) and disposed when that scope
/// is dropped.
///
/// ```
/// # use sindon_reactive::{Effect, Signal};
/// let count = Signal::new(0);
/// Effect::new(move || {
///     println!("count is {}", count.get());
/// });
/// count.set(1); // prints "count is 1"
/// ```
pub struct Effect;

impl Effect {
    /// Create and immediately run a new effect.
    ///
    /// The callback is executed once right away (to capture dependencies),
    /// then re-executed whenever any dependency changes.
    ///
    /// This intentionally returns `()` (not `Self`) — effects are managed
    /// by the enclosing `Scope` and never held directly by the caller.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(callback: impl FnMut() + 'static) {
        RUNTIME.with(|rt| {
            rt.create_effect(callback);
        });
    }
}
