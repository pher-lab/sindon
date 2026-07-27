use crate::node::{AnySecureCompute, ReactiveId};
use crate::runtime::RUNTIME;
use crate::scope::on_cleanup;
use crate::secure_signal::{drop_and_zero, with_arena};
use sindon_security::ArenaSlot;
use std::marker::PhantomData;
use zeroize::Zeroize;

// ── AnySecureCompute implementation ────────────────────────────────

struct SecureComputeFn<T, F> {
    f: F,
    _marker: PhantomData<T>,
}

impl<T, F> AnySecureCompute for SecureComputeFn<T, F>
where
    T: Zeroize + PartialEq + 'static,
    F: FnMut() -> T,
{
    fn recompute(&mut self, slot: ArenaSlot, has_value: bool) -> bool {
        // 1. Run compute (may read other signals/secure signals)
        let new_value = (self.f)();

        // 2. Compare with old arena value, write if different
        with_arena(|arena| {
            let ptr = arena.slot_ptr_mut(slot) as *mut T;

            if has_value {
                let old = unsafe { &*ptr };
                if *old == new_value {
                    // Value unchanged — discard new, keep old
                    drop(new_value);
                    return false;
                }
                // Zeroize and drop old value
                unsafe {
                    std::ptr::drop_in_place(ptr);
                    std::ptr::write_bytes(ptr as *mut u8, 0, std::mem::size_of::<T>());
                }
            }

            // Write new value
            unsafe {
                std::ptr::write(ptr, new_value);
            }
            true
        })
    }
}

// ── SecureMemo ─────────────────────────────────────────────────────

/// A cached derivation whose value is stored in a mlock'd arena.
///
/// Like [`Memo`](crate::Memo) but for sensitive data: the cached value
/// lives in the secure arena and is zeroized on recomputation. Access
/// is closure-based (`expose`).
///
/// If the recomputed value equals the old value (via `PartialEq`),
/// downstream subscribers are not notified (equality skip).
///
/// `SecureMemo<T>` is `Copy`.
#[derive(Debug)]
pub struct SecureMemo<T: Zeroize> {
    id: ReactiveId,
    _marker: PhantomData<T>,
}

impl<T: Zeroize> Clone for SecureMemo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Zeroize> Copy for SecureMemo<T> {}

impl<T: Zeroize + PartialEq + 'static> SecureMemo<T> {
    /// Create a new secure memo with the given computation.
    ///
    /// The computation runs lazily on first read. The cached result
    /// is stored in the mlock'd arena.
    pub fn new(compute: impl FnMut() -> T + 'static) -> Self {
        let slot = with_arena(|arena| {
            arena
                .alloc(std::mem::size_of::<T>(), std::mem::align_of::<T>())
                .expect("secure arena out of memory")
        });

        let compute_box: Box<dyn AnySecureCompute> = Box::new(SecureComputeFn {
            f: compute,
            _marker: PhantomData::<T>,
        });

        let id =
            RUNTIME.with(|rt| rt.create_secure_memo_node(compute_box, slot, drop_and_zero::<T>));

        // Register scope cleanup
        let cleanup_slot = slot;
        on_cleanup(move || {
            with_arena(|arena| {
                let ptr = arena.slot_ptr_mut(cleanup_slot);
                unsafe {
                    drop_and_zero::<T>(ptr);
                }
                arena.dealloc(cleanup_slot);
            });
        });

        SecureMemo {
            id,
            _marker: PhantomData,
        }
    }

    /// Access the cached value inside a closure.
    ///
    /// Triggers recomputation if stale. Registers a dependency if
    /// called inside an Effect or Memo.
    pub fn expose<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // update_if_needed + track
        let slot = RUNTIME.with(|rt| rt.read_secure_memo(self.id));
        with_arena(|arena| {
            let ptr = arena.slot_ptr(slot) as *const T;
            f(unsafe { &*ptr })
        })
    }

    /// Return the reactive node id (for testing / debugging).
    pub fn id(&self) -> ReactiveId {
        self.id
    }
}
