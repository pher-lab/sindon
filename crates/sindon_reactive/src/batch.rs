use crate::runtime::RUNTIME;

/// Execute a closure with batched updates.
///
/// During the closure, signal writes accumulate dirty marks but effects are
/// not flushed. After the closure returns (or panics), all pending effects
/// run exactly once.
///
/// ```ignore
/// let a = Signal::new(0);
/// let b = Signal::new(0);
///
/// // Without batch: effect runs twice (once per set)
/// // With batch: effect runs once after both sets
/// batch(|| {
///     a.set(1);
///     b.set(2);
/// });
/// ```
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    RUNTIME.with(|rt| rt.start_batch());

    // Use catch_unwind to guarantee end_batch runs even on panic
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    RUNTIME.with(|rt| rt.end_batch());

    match result {
        Ok(val) => val,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
