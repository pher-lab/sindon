use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use shroud_security::SecureString;
use std::hint::black_box;

const SIZES: &[usize] = &[16, 64, 256, 1024];

fn construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("construction");
    for &size in SIZES {
        let input = "a".repeat(size);
        group.bench_with_input(BenchmarkId::new("SecureString", size), &input, |b, s| {
            b.iter(|| SecureString::new(black_box(s.as_str())));
        });
        group.bench_with_input(BenchmarkId::new("String", size), &input, |b, s| {
            b.iter(|| String::from(black_box(s.as_str())));
        });
    }
    group.finish();
}

fn drop_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("drop");
    for &size in SIZES {
        let input = "a".repeat(size);
        group.bench_with_input(BenchmarkId::new("SecureString", size), &input, |b, s| {
            b.iter_batched(
                || SecureString::new(s.as_str()),
                drop,
                BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new("String", size), &input, |b, s| {
            b.iter_batched(
                || String::from(s.as_str()),
                drop,
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn expose(c: &mut Criterion) {
    let s = SecureString::new("hello world");
    c.bench_function("SecureString::expose", |b| {
        b.iter(|| s.expose(|inner| black_box(inner.len())));
    });
}

criterion_group!(benches, construction, drop_cost, expose);
criterion_main!(benches);
