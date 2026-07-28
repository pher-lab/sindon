use crate::node::ReactiveId;
use crate::runtime::RUNTIME;
use crate::scope::on_cleanup;
use sindon_security::{ArenaSlot, DEFAULT_ARENA_CAPACITY, SecureArena};
use std::cell::OnceCell;
use std::marker::PhantomData;
use zeroize::Zeroize;

// ── Thread-local arena ─────────────────────────────────────────────

thread_local! {
    static SECURE_ARENA: OnceCell<SecureArena> = const { OnceCell::new() };
}

/// Access the thread-local secure arena.
///
/// Framework wiring, not app API: `SecureSignal` / `SecureMemo` are how an
/// app reaches the arena. Public only so the crate's own residue tests can
/// inspect slot occupancy from outside the crate.
#[doc(hidden)]
pub fn with_arena<R>(f: impl FnOnce(&SecureArena) -> R) -> R {
    SECURE_ARENA.with(|cell| {
        let arena = cell.get_or_init(|| {
            SecureArena::new(DEFAULT_ARENA_CAPACITY)
                .expect("failed to create secure arena — check mlock limits")
        });
        f(arena)
    })
}

// ── Drop helper ────────────────────────────────────────────────────

/// Type-erased cleanup: drop the value (runs T::drop which may zeroize heap
/// data), then zero the struct bytes in the arena.
///
/// # Safety
/// `ptr` must point to a valid, initialized `T`.
pub(crate) unsafe fn drop_and_zero<T: Zeroize>(ptr: *mut u8) {
    let typed = ptr as *mut T;
    // drop_in_place runs T::drop (zeroizes heap data if T: ZeroizeOnDrop)
    unsafe { std::ptr::drop_in_place(typed) };
    // Zero the struct bytes left in the arena slot
    unsafe { std::ptr::write_bytes(ptr, 0, std::mem::size_of::<T>()) };
}

// ── SecureSignal ───────────────────────────────────────────────────

/// A reactive signal whose value is stored in a mlock'd arena.
///
/// Access is closure-based (`expose`) to prevent reference escape.
/// Old values are zeroized on `set()`, and the arena slot is zeroized
/// when the owning scope is disposed.
///
/// `SecureSignal<T>` is `Copy` — it's a lightweight handle.
#[derive(Debug)]
pub struct SecureSignal<T: Zeroize> {
    id: ReactiveId,
    slot: ArenaSlot,
    _marker: PhantomData<T>,
}

impl<T: Zeroize> Clone for SecureSignal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Zeroize> Copy for SecureSignal<T> {}

impl<T: Zeroize + 'static> SecureSignal<T> {
    /// Create a new secure signal with the given initial value.
    ///
    /// The value is written into the mlock'd arena. A cleanup is
    /// registered on the current scope (if any) to zeroize the slot
    /// on disposal.
    pub fn new(value: T) -> Self {
        let slot = with_arena(|arena| {
            arena
                .alloc(std::mem::size_of::<T>(), std::mem::align_of::<T>())
                .expect("secure arena out of memory")
        });

        // Write value into the arena slot
        with_arena(|arena| {
            let ptr = arena.slot_ptr_mut(slot) as *mut T;
            unsafe {
                std::ptr::write(ptr, value);
            }
        });

        let id = RUNTIME.with(|rt| rt.create_secure_signal_node(slot, drop_and_zero::<T>));

        // Register scope cleanup: zeroize + dealloc on scope disposal
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

        SecureSignal {
            id,
            slot,
            _marker: PhantomData,
        }
    }

    /// Access the value inside a closure.
    ///
    /// Registers a dependency if called inside an Effect or Memo.
    /// The reference cannot escape the closure.
    pub fn expose<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        RUNTIME.with(|rt| rt.track_read(self.id));
        with_arena(|arena| {
            let ptr = arena.slot_ptr(self.slot) as *const T;
            f(unsafe { &*ptr })
        })
    }

    /// Replace the value, zeroizing the old one.
    ///
    /// The old value's destructor runs (which zeroizes heap data if
    /// `T: ZeroizeOnDrop`), then the struct bytes in the arena are zeroed,
    /// then the new value is written.
    pub fn set(&self, value: T) {
        with_arena(|arena| {
            let ptr = arena.slot_ptr_mut(self.slot) as *mut T;
            unsafe {
                // Drop old value (runs T::drop → zeroizes heap data)
                std::ptr::drop_in_place(ptr);
                // Zero the struct bytes
                std::ptr::write_bytes(ptr as *mut u8, 0, std::mem::size_of::<T>());
                // Write new value
                std::ptr::write(ptr, value);
            }
        });
        RUNTIME.with(|rt| rt.notify_write(self.id));
    }

    /// Return the reactive node id (for testing / debugging).
    pub fn id(&self) -> ReactiveId {
        self.id
    }

    /// Return the arena slot (for testing / debugging).
    pub fn slot(&self) -> ArenaSlot {
        self.slot
    }
}
