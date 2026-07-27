use crate::node::ReactiveId;
use crate::runtime::RUNTIME;
use std::marker::PhantomData;

/// A reactive signal holding a value of type `T`.
///
/// Signals are the atomic unit of reactive state. Reading a signal inside
/// an Effect or Memo automatically registers a dependency. Writing triggers
/// re-evaluation of all subscribers.
///
/// `Signal<T>` is `Copy` — it's a lightweight handle into the reactive runtime.
#[derive(Debug)]
pub struct Signal<T> {
    id: ReactiveId,
    _marker: PhantomData<T>,
}

// Manual impls so T doesn't need Copy/Clone.
impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}

impl<T: 'static> Signal<T> {
    /// Create a new signal with the given initial value.
    pub fn new(value: T) -> Self {
        let id = RUNTIME.with(|rt| rt.create_signal(value));
        Signal {
            id,
            _marker: PhantomData,
        }
    }

    /// Read the value by borrowing it inside a closure.
    ///
    /// Registers a dependency if called inside an Effect or Memo.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        RUNTIME.with(|rt| rt.read_signal::<T, R>(self.id, f))
    }

    /// Replace the signal's value, notifying all subscribers.
    pub fn set(&self, value: T) {
        RUNTIME.with(|rt| rt.write_signal::<T>(self.id, value));
    }

    /// Mutate the signal's value in-place, notifying all subscribers.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        RUNTIME.with(|rt| rt.update_signal::<T>(self.id, f));
    }

    /// Return the reactive node id (for testing / debugging).
    pub fn id(&self) -> ReactiveId {
        self.id
    }
}

impl<T: Copy + 'static> Signal<T> {
    /// Get a copy of the value.
    ///
    /// Registers a dependency if called inside an Effect or Memo.
    pub fn get(&self) -> T {
        self.with(|v| *v)
    }
}

impl<T: Clone + 'static> Signal<T> {
    /// Get a clone of the value.
    ///
    /// Registers a dependency if called inside an Effect or Memo.
    pub fn get_clone(&self) -> T {
        self.with(|v| v.clone())
    }
}
