use shroud_reactive::*;
use std::cell::Cell;
use std::rc::Rc;

// ── Signal basics ──────────────────────────────────────────────────

#[test]
fn signal_get_set() {
    let count = Signal::new(0);
    assert_eq!(count.get(), 0);

    count.set(42);
    assert_eq!(count.get(), 42);
}

#[test]
fn signal_update() {
    let count = Signal::new(10);
    count.update(|v| *v += 5);
    assert_eq!(count.get(), 15);
}

#[test]
fn signal_with_non_copy() {
    let name = Signal::new(String::from("hello"));
    let len = name.with(|s| s.len());
    assert_eq!(len, 5);

    name.set(String::from("world!"));
    name.with(|s| assert_eq!(s, "world!"));
}

#[test]
fn signal_is_copy() {
    let a = Signal::new(1);
    let b = a; // Copy
    a.set(2);
    assert_eq!(b.get(), 2); // same underlying signal
}

// ── Effect basics ──────────────────────────────────────────────────

#[test]
fn effect_runs_on_creation() {
    let ran = Rc::new(Cell::new(false));
    let ran_clone = ran.clone();

    Effect::new(move || {
        ran_clone.set(true);
    });

    assert!(ran.get());
}

#[test]
fn effect_tracks_signal() {
    let count = Signal::new(0);
    let observed = Rc::new(Cell::new(0));
    let observed_clone = observed.clone();

    Effect::new(move || {
        observed_clone.set(count.get());
    });

    assert_eq!(observed.get(), 0);

    count.set(5);
    assert_eq!(observed.get(), 5);

    count.set(100);
    assert_eq!(observed.get(), 100);
}

#[test]
fn effect_tracks_multiple_signals() {
    let a = Signal::new(1);
    let b = Signal::new(2);
    let sum = Rc::new(Cell::new(0));
    let sum_clone = sum.clone();

    Effect::new(move || {
        sum_clone.set(a.get() + b.get());
    });

    assert_eq!(sum.get(), 3);

    a.set(10);
    assert_eq!(sum.get(), 12);

    b.set(20);
    assert_eq!(sum.get(), 30);
}

// ── Memo basics ────────────────────────────────────────────────────

#[test]
fn memo_caches_computation() {
    let count = Signal::new(3);
    let compute_count = Rc::new(Cell::new(0));
    let cc = compute_count.clone();

    let doubled = Memo::new(move || {
        cc.set(cc.get() + 1);
        count.get() * 2
    });

    // First read triggers computation
    assert_eq!(doubled.get(), 6);
    assert_eq!(compute_count.get(), 1);

    // Second read without signal change — cached
    assert_eq!(doubled.get(), 6);
    assert_eq!(compute_count.get(), 1);

    // Signal changes — recomputes on next read
    count.set(5);
    assert_eq!(doubled.get(), 10);
    assert_eq!(compute_count.get(), 2);
}

#[test]
fn memo_equality_skip() {
    let input = Signal::new(5);
    let effect_count = Rc::new(Cell::new(0));
    let ec = effect_count.clone();

    // Memo that clamps to [0, 10]
    let clamped = Memo::new(move || input.get().clamp(0, 10));

    Effect::new(move || {
        let _val = clamped.get();
        ec.set(ec.get() + 1);
    });

    // Initial run
    assert_eq!(effect_count.get(), 1);

    // Set to 15 → clamped is still 10 (different from 5, so effect runs)
    input.set(15);
    assert_eq!(clamped.get(), 10);
    assert_eq!(effect_count.get(), 2);

    // Set to 20 → clamped is still 10 → equality skip, effect does NOT run
    input.set(20);
    assert_eq!(clamped.get(), 10);
    assert_eq!(effect_count.get(), 2);
}

// ── Diamond dependency ─────────────────────────────────────────────

#[test]
fn diamond_dependency_effect_runs_once() {
    //     a (signal)
    //    / \
    //   b   c  (memos)
    //    \ /
    //     d  (effect)
    let a = Signal::new(1);

    let b = Memo::new(move || a.get() + 1);
    let c = Memo::new(move || a.get() * 10);

    let effect_count = Rc::new(Cell::new(0));
    let ec = effect_count.clone();
    let observed = Rc::new(Cell::new((0, 0)));
    let obs = observed.clone();

    Effect::new(move || {
        ec.set(ec.get() + 1);
        obs.set((b.get(), c.get()));
    });

    assert_eq!(effect_count.get(), 1);
    assert_eq!(observed.get(), (2, 10));

    // Change a → both b and c change, but effect should run exactly once
    a.set(5);
    assert_eq!(effect_count.get(), 2);
    assert_eq!(observed.get(), (6, 50));
}

// ── Conditional dependencies ───────────────────────────────────────

#[test]
fn conditional_dependencies() {
    let flag = Signal::new(true);
    let a = Signal::new(1);
    let b = Signal::new(2);

    let result = Rc::new(Cell::new(0));
    let res = result.clone();
    let effect_count = Rc::new(Cell::new(0));
    let ec = effect_count.clone();

    Effect::new(move || {
        ec.set(ec.get() + 1);
        if flag.get() {
            res.set(a.get());
        } else {
            res.set(b.get());
        }
    });

    assert_eq!(result.get(), 1);
    assert_eq!(effect_count.get(), 1);

    // Changing a triggers (currently tracked)
    a.set(10);
    assert_eq!(result.get(), 10);
    assert_eq!(effect_count.get(), 2);

    // Changing b does NOT trigger (not currently tracked)
    b.set(20);
    assert_eq!(result.get(), 10);
    assert_eq!(effect_count.get(), 2);

    // Switch branch — now tracks b instead of a
    flag.set(false);
    assert_eq!(result.get(), 20);
    assert_eq!(effect_count.get(), 3);

    // Now changing a does NOT trigger
    a.set(99);
    assert_eq!(result.get(), 20);
    assert_eq!(effect_count.get(), 3);

    // But changing b does
    b.set(30);
    assert_eq!(result.get(), 30);
    assert_eq!(effect_count.get(), 4);
}

// ── Batch updates ──────────────────────────────────────────────────

#[test]
fn batch_defers_effects() {
    let a = Signal::new(0);
    let b = Signal::new(0);

    let effect_count = Rc::new(Cell::new(0));
    let ec = effect_count.clone();
    let observed = Rc::new(Cell::new((0, 0)));
    let obs = observed.clone();

    Effect::new(move || {
        ec.set(ec.get() + 1);
        obs.set((a.get(), b.get()));
    });

    assert_eq!(effect_count.get(), 1);

    batch(|| {
        a.set(1);
        b.set(2);
        // Effect has NOT run yet during batch
        assert_eq!(effect_count.get(), 1);
    });

    // After batch: effect ran exactly once
    assert_eq!(effect_count.get(), 2);
    assert_eq!(observed.get(), (1, 2));
}

#[test]
fn batch_returns_value() {
    let result = batch(|| 42);
    assert_eq!(result, 42);
}

// ── Scope and cleanup ──────────────────────────────────────────────

#[test]
fn scope_disposes_children() {
    let outer = Signal::new(0);
    let effect_count = Rc::new(Cell::new(0));
    let ec = effect_count.clone();

    {
        let scope = Scope::new();
        scope.run(|| {
            let ec2 = ec.clone();
            Effect::new(move || {
                let _val = outer.get();
                ec2.set(ec2.get() + 1);
            });
        });

        assert_eq!(effect_count.get(), 1);
        outer.set(1);
        assert_eq!(effect_count.get(), 2);

        // scope dropped here
    }

    // Effect should no longer fire after scope disposal
    outer.set(2);
    assert_eq!(effect_count.get(), 2);
}

#[test]
fn scope_cleanup_runs_in_reverse() {
    let order = Rc::new(RefCell::new(Vec::new()));

    {
        let scope = Scope::new();
        let o1 = order.clone();
        let o2 = order.clone();
        let o3 = order.clone();

        scope.on_cleanup(move || o1.borrow_mut().push(1));
        scope.on_cleanup(move || o2.borrow_mut().push(2));
        scope.on_cleanup(move || o3.borrow_mut().push(3));
    }

    assert_eq!(*order.borrow(), vec![3, 2, 1]);
}

use std::cell::RefCell;

#[test]
fn nested_scopes_dispose_recursively() {
    let outer_cleaned = Rc::new(Cell::new(false));
    let inner_cleaned = Rc::new(Cell::new(false));
    let signal_outside = Signal::new(0);
    let effect_count = Rc::new(Cell::new(0));

    {
        let scope = Scope::new();
        let oc = outer_cleaned.clone();

        scope.on_cleanup(move || oc.set(true));

        scope.run(|| {
            let inner_scope = Scope::new();
            let ic = inner_cleaned.clone();

            inner_scope.on_cleanup(move || ic.set(true));

            let ec = effect_count.clone();
            inner_scope.run(|| {
                Effect::new(move || {
                    let _val = signal_outside.get();
                    ec.set(ec.get() + 1);
                });
            });

            // Don't drop inner_scope — it's owned by the outer scope
            std::mem::forget(inner_scope);
        });

        assert_eq!(effect_count.get(), 1);
        signal_outside.set(1);
        assert_eq!(effect_count.get(), 2);
    }

    // Both scopes cleaned up
    assert!(outer_cleaned.get());
    assert!(inner_cleaned.get());

    // Effect no longer fires
    signal_outside.set(2);
    assert_eq!(effect_count.get(), 2);
}

#[test]
fn on_cleanup_free_function() {
    let cleaned = Rc::new(Cell::new(false));
    let cc = cleaned.clone();

    {
        let scope = Scope::new();
        scope.run(move || {
            on_cleanup(move || cc.set(true));
        });
    }

    assert!(cleaned.get());
}

// ── Memo chain ─────────────────────────────────────────────────────

#[test]
fn memo_chain() {
    let base = Signal::new(2);
    let doubled = Memo::new(move || base.get() * 2);
    let quadrupled = Memo::new(move || doubled.get() * 2);

    assert_eq!(quadrupled.get(), 8);

    base.set(3);
    assert_eq!(quadrupled.get(), 12);
}

// ── Effect inside effect (signal set during effect) ────────────────

#[test]
fn signal_set_inside_effect() {
    let source = Signal::new(1);
    let derived = Signal::new(0);

    // First effect writes to derived based on source
    let d = derived;
    Effect::new(move || {
        d.set(source.get() * 10);
    });

    let observed = Rc::new(Cell::new(0));
    let obs = observed.clone();

    // Second effect reads derived
    Effect::new(move || {
        obs.set(derived.get());
    });

    assert_eq!(observed.get(), 10);

    source.set(5);
    assert_eq!(observed.get(), 50);
}

// ── Reactive<T> ────────────────────────────────────────────────────
//
// `Reactive<T>` wraps either a static value or a closure so widget
// attributes can accept both transparently via `impl Into<Reactive<T>>`.

#[test]
fn reactive_static_from_value() {
    let r: Reactive<u32> = 42.into();
    assert_eq!(r.get(), 42);
    // Re-read returns the same value (no closure invocation).
    assert_eq!(r.get(), 42);
}

#[test]
fn reactive_static_from_string() {
    // Non-Copy T path — `get()` clones the held value.
    let r: Reactive<String> = String::from("hello").into();
    assert_eq!(r.get(), "hello");
    assert_eq!(r.get(), "hello");
}

#[test]
fn reactive_dynamic_from_signal_tracks_updates() {
    let s = Signal::new(7);
    let r: Reactive<i32> = s.into();
    assert_eq!(r.get(), 7);

    // Mutating the signal is visible through the Reactive handle on the
    // next `get()` — this is the whole point of the Dynamic variant.
    s.set(9);
    assert_eq!(r.get(), 9);
}

#[test]
fn reactive_dynamic_from_memo_tracks_updates() {
    let src = Signal::new(3);
    let m = Memo::new(move || src.get() * 2);
    let r: Reactive<i32> = m.into();

    assert_eq!(r.get(), 6);
    src.set(10);
    assert_eq!(r.get(), 20);
}

#[test]
fn reactive_derive_from_closure() {
    // `Reactive::derive` is the escape hatch when neither `T` nor
    // `Signal`/`Memo` conversions apply (e.g., combining multiple signals).
    let a = Signal::new(2);
    let b = Signal::new(3);
    let r = Reactive::derive(move || a.get() + b.get());
    assert_eq!(r.get(), 5);

    a.set(10);
    assert_eq!(r.get(), 13);
    b.set(100);
    assert_eq!(r.get(), 110);
}

#[test]
fn reactive_clone_shares_dynamic_closure() {
    // Cloning a `Reactive::Dynamic` must not re-allocate or re-run the
    // closure — the internal Rc is shared. We observe this by counting
    // closure invocations: each `get()` bumps a counter exactly once,
    // regardless of which clone is read.
    let calls = Rc::new(Cell::new(0));
    let c = calls.clone();
    let r: Reactive<i32> = Reactive::derive(move || {
        c.set(c.get() + 1);
        42
    });
    let r2 = r.clone();

    assert_eq!(r.get(), 42);
    assert_eq!(calls.get(), 1);
    assert_eq!(r2.get(), 42);
    assert_eq!(calls.get(), 2);
}

#[test]
fn reactive_clone_of_static_is_independent_value() {
    // Cloning Static produces a separate owned T — writing into one
    // handle (if we had such an API) would not affect the other. This
    // test documents the semantic: Static is plain-old-data.
    let r: Reactive<String> = String::from("abc").into();
    let r2 = r.clone();
    assert_eq!(r.get(), "abc");
    assert_eq!(r2.get(), "abc");
}
