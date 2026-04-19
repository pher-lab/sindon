use criterion::{Criterion, criterion_group, criterion_main};
use shroud_reactive::{Effect, Memo, Signal, batch};
use std::cell::Cell;
use std::hint::black_box;
use std::rc::Rc;

fn signal_set_noop(c: &mut Criterion) {
    c.bench_function("signal_set_noop", |b| {
        let sig = Signal::new(0u64);
        let mut i = 0u64;
        b.iter(|| {
            i = i.wrapping_add(1);
            sig.set(black_box(i));
        });
    });
}

fn signal_set_with_effect(c: &mut Criterion) {
    c.bench_function("signal_set_with_effect", |b| {
        let sig = Signal::new(0u64);
        let observed = Rc::new(Cell::new(0u64));
        {
            let observed = observed.clone();
            Effect::new(move || {
                observed.set(sig.get());
            });
        }
        let mut i = 0u64;
        b.iter(|| {
            i = i.wrapping_add(1);
            sig.set(black_box(i));
        });
    });
}

fn memo_get_cached(c: &mut Criterion) {
    c.bench_function("memo_get_cached", |b| {
        let sig = Signal::new(1u64);
        let memo = Memo::new(move || sig.get().wrapping_mul(2));
        black_box(memo.get());
        b.iter(|| black_box(memo.get()));
    });
}

fn memo_chain_recompute(c: &mut Criterion) {
    c.bench_function("memo_chain_recompute", |b| {
        let sig = Signal::new(0u64);
        let m1 = Memo::new(move || sig.get().wrapping_add(1));
        let m2 = Memo::new(move || m1.get().wrapping_add(1));
        let m3 = Memo::new(move || m2.get().wrapping_add(1));
        let mut i = 0u64;
        b.iter(|| {
            i = i.wrapping_add(1);
            sig.set(black_box(i));
            black_box(m3.get());
        });
    });
}

fn batch_vs_individual(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_vs_individual_100_sets");

    group.bench_function("individual", |b| {
        let sig = Signal::new(0u64);
        let observed = Rc::new(Cell::new(0u64));
        {
            let observed = observed.clone();
            Effect::new(move || {
                observed.set(sig.get());
            });
        }
        b.iter(|| {
            for i in 0..100u64 {
                sig.set(black_box(i));
            }
        });
    });

    group.bench_function("batched", |b| {
        let sig = Signal::new(0u64);
        let observed = Rc::new(Cell::new(0u64));
        {
            let observed = observed.clone();
            Effect::new(move || {
                observed.set(sig.get());
            });
        }
        b.iter(|| {
            batch(|| {
                for i in 0..100u64 {
                    sig.set(black_box(i));
                }
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    signal_set_noop,
    signal_set_with_effect,
    memo_get_cached,
    memo_chain_recompute,
    batch_vs_individual,
);
criterion_main!(benches);
