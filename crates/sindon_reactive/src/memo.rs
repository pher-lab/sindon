use crate::node::ReactiveId;
use crate::runtime::RUNTIME;
use std::marker::PhantomData;

/// A cached, derived computation that re-evaluates only when its
/// dependencies change.
///
/// If the recomputed value is equal to the previous value (via `PartialEq`),
/// downstream subscribers are NOT notified — this is the "equality skip"
/// optimization that prevents unnecessary work in diamond dependency graphs.
///
/// `Memo<T>` is `Copy`.
#[derive(Debug)]
pub struct Memo<T> {
    id: ReactiveId,
    _marker: PhantomData<T>,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T: PartialEq + 'static> Memo<T> {
    /// Create a new memo with the given computation.
    ///
    /// The computation is not evaluated immediately — it runs lazily on
    /// first read.
    pub fn new(compute: impl FnMut() -> T + 'static) -> Self {
        let id = RUNTIME.with(|rt| rt.create_memo::<T>(compute));
        Memo {
            id,
            _marker: PhantomData,
        }
    }

    /// Read the memo's cached value by borrowing it inside a closure.
    ///
    /// Triggers recomputation if the memo is stale. Registers a dependency
    /// if called inside an Effect or Memo.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        RUNTIME.with(|rt| rt.read_memo::<T, R>(self.id, f))
    }

    /// Return the reactive node id (for testing / debugging).
    pub fn id(&self) -> ReactiveId {
        self.id
    }
}

impl<T: Copy + PartialEq + 'static> Memo<T> {
    /// Get a copy of the cached value.
    pub fn get(&self) -> T {
        self.with(|v| *v)
    }
}

impl<T: Clone + PartialEq + 'static> Memo<T> {
    /// Get a clone of the cached value.
    pub fn get_clone(&self) -> T {
        self.with(|v| v.clone())
    }
}
