use shroud_reactive::*;
use shroud_security::SecureArena;
use std::cell::Cell;
use std::rc::Rc;
use zeroize::Zeroize;

// ── Arena basics ───────────────────────────────────────────────────

#[test]
fn arena_alloc_and_dealloc() {
    let arena = SecureArena::new(4096).unwrap();

    // Allocate a 64-byte class slot
    let slot = arena.alloc(32, 8).unwrap();
    assert_eq!(slot.class_size(), 64);

    // Write data
    let ptr = arena.slot_ptr_mut(slot) as *mut u32;
    unsafe { ptr.write(0xDEAD_BEEF) };
    assert_eq!(unsafe { *ptr }, 0xDEAD_BEEF);

    // Dealloc zeroizes
    arena.dealloc(slot);
    assert_eq!(unsafe { *ptr }, 0);
}

#[test]
fn arena_size_classes() {
    let arena = SecureArena::new(64 * 1024).unwrap();

    let s1 = arena.alloc(1, 1).unwrap();
    assert_eq!(s1.class_size(), 64);

    let s2 = arena.alloc(65, 1).unwrap();
    assert_eq!(s2.class_size(), 256);

    let s3 = arena.alloc(257, 1).unwrap();
    assert_eq!(s3.class_size(), 1024);

    let s4 = arena.alloc(1025, 1).unwrap();
    assert_eq!(s4.class_size(), 4096);
}

#[test]
fn arena_too_large_allocation() {
    let arena = SecureArena::new(4096).unwrap();
    let result = arena.alloc(4097, 1);
    assert!(result.is_err());
}

#[test]
fn arena_free_list_reuse() {
    let arena = SecureArena::new(4096).unwrap();

    let slot1 = arena.alloc(32, 8).unwrap();
    let offset1 = slot1.offset();
    arena.dealloc(slot1);

    // Next allocation of the same class should reuse the freed slot
    let slot2 = arena.alloc(32, 8).unwrap();
    assert_eq!(slot2.offset(), offset1);
}

#[test]
fn arena_zeroize_all() {
    let arena = SecureArena::new(4096).unwrap();

    let slot = arena.alloc(32, 8).unwrap();
    let ptr = arena.slot_ptr_mut(slot) as *mut u64;
    unsafe { ptr.write(0xFFFF_FFFF_FFFF_FFFF) };

    arena.zeroize_all();
    assert_eq!(unsafe { *ptr }, 0);
    assert_eq!(arena.bump_used(), 0);
}

// ── SecureSignal basics ────────────────────────────────────────────

/// A simple Zeroize type for testing.
#[derive(Debug, Clone, PartialEq)]
struct SecretKey([u8; 32]);

impl Zeroize for SecretKey {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

#[test]
fn secure_signal_expose() {
    let key = SecretKey([0xAB; 32]);
    let signal = SecureSignal::new(key);

    signal.expose(|k| {
        assert_eq!(k.0[0], 0xAB);
        assert_eq!(k.0[31], 0xAB);
    });
}

#[test]
fn secure_signal_set_zeroizes_old() {
    let signal = SecureSignal::new(SecretKey([0xFF; 32]));

    // Get the arena pointer for verification
    let slot = signal.slot();

    // Set new value
    signal.set(SecretKey([0x42; 32]));

    // New value should be readable
    signal.expose(|k| {
        assert_eq!(k.0[0], 0x42);
    });

    // The slot should contain the new value, not the old
    shroud_reactive::secure_signal::with_arena(|arena| {
        let ptr = arena.slot_ptr(slot) as *const SecretKey;
        let val = unsafe { &*ptr };
        assert_eq!(val.0[0], 0x42);
        // Old value (0xFF) should be gone
        assert_ne!(val.0[0], 0xFF);
    });
}

#[test]
fn secure_signal_is_copy() {
    let a = SecureSignal::new(SecretKey([1; 32]));
    let b = a; // Copy
    a.expose(|k| assert_eq!(k.0[0], 1));
    b.expose(|k| assert_eq!(k.0[0], 1));
}

// ── SecureSignal + reactive system ─────────────────────────────────

#[test]
fn secure_signal_triggers_effect() {
    let signal = SecureSignal::new(SecretKey([0; 32]));
    let observed = Rc::new(Cell::new(0u8));
    let obs = observed.clone();

    Effect::new(move || {
        signal.expose(|k| {
            obs.set(k.0[0]);
        });
    });

    assert_eq!(observed.get(), 0);

    signal.set(SecretKey([42; 32]));
    assert_eq!(observed.get(), 42);
}

#[test]
fn secure_signal_with_memo() {
    let signal = SecureSignal::new(SecretKey([5; 32]));

    // Non-secure memo derived from secure signal
    let sum = Memo::new(move || signal.expose(|k| k.0.iter().map(|b| *b as u32).sum::<u32>()));

    assert_eq!(sum.get(), 5 * 32);

    signal.set(SecretKey([10; 32]));
    assert_eq!(sum.get(), 10 * 32);
}

// ── SecureMemo ─────────────────────────────────────────────────────

#[test]
fn secure_memo_caches_computation() {
    let input = Signal::new(3u32);
    let compute_count = Rc::new(Cell::new(0u32));
    let cc = compute_count.clone();

    let derived = SecureMemo::new(move || {
        cc.set(cc.get() + 1);
        SecretKey([input.get() as u8; 32])
    });

    // First read triggers computation
    derived.expose(|k| assert_eq!(k.0[0], 3));
    assert_eq!(compute_count.get(), 1);

    // Second read without input change — cached
    derived.expose(|k| assert_eq!(k.0[0], 3));
    assert_eq!(compute_count.get(), 1);

    // Input changes — recomputes on next read
    input.set(7);
    derived.expose(|k| assert_eq!(k.0[0], 7));
    assert_eq!(compute_count.get(), 2);
}

#[test]
fn secure_memo_equality_skip() {
    let input = Signal::new(5u32);
    let effect_count = Rc::new(Cell::new(0u32));
    let ec = effect_count.clone();

    // SecureMemo that clamps to [0, 10]
    let clamped = SecureMemo::new(move || {
        let v = input.get().clamp(0, 10) as u8;
        SecretKey([v; 32])
    });

    Effect::new(move || {
        clamped.expose(|_| {});
        ec.set(ec.get() + 1);
    });

    assert_eq!(effect_count.get(), 1);

    // Set to 15 → clamped value changes from 5 to 10
    input.set(15);
    assert_eq!(effect_count.get(), 2);

    // Set to 20 → clamped is still 10 → equality skip
    input.set(20);
    assert_eq!(effect_count.get(), 2);
}

// ── SecureSignal + Scope cleanup ───────────────────────────────────

#[test]
fn scope_disposes_secure_signal() {
    let effect_count = Rc::new(Cell::new(0u32));
    let ec = effect_count.clone();
    let outer = Signal::new(0u32);

    {
        let scope = Scope::new();
        scope.run(|| {
            let secure = SecureSignal::new(SecretKey([1; 32]));
            let ec2 = ec.clone();

            Effect::new(move || {
                let _trigger = outer.get();
                secure.expose(|_| {});
                ec2.set(ec2.get() + 1);
            });
        });

        assert_eq!(effect_count.get(), 1);
        outer.set(1);
        assert_eq!(effect_count.get(), 2);
    }

    // After scope disposal, effect should no longer fire
    outer.set(2);
    assert_eq!(effect_count.get(), 2);
}

// ── SecureSignal with u8 array (pure arena storage) ────────────────

#[test]
fn secure_signal_zeroize_verified() {
    // Use a raw byte array to verify zeroization
    let signal = SecureSignal::new([0xFFu8; 64]);
    let slot = signal.slot();

    // Verify initial value
    signal.expose(|v| assert_eq!(v[0], 0xFF));

    // Set new value — old should be zeroized
    signal.set([0x42u8; 64]);
    signal.expose(|v| assert_eq!(v[0], 0x42));

    // Read raw arena memory — should have new value, not old
    shroud_reactive::secure_signal::with_arena(|arena| {
        let ptr = arena.slot_ptr(slot);
        let bytes = unsafe { std::slice::from_raw_parts(ptr, 64) };
        assert!(bytes.iter().all(|&b| b == 0x42));
    });
}
