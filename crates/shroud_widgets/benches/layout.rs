//! Layout-pass cost as a function of tree size.
//!
//! Written to chase a measured regression, not in the abstract: a knot
//! session with a long note open spent **19 ms of a 23 ms frame inside
//! layout**, on frames that shaped no new text at all (`shapes=0` in the
//! `SHROUD_PERF` log — every shaping call was a cache hit). That rules out
//! shaping itself and points at the per-frame work `compute_layout_with_measure`
//! does around it, which is what these benchmarks isolate.
//!
//! The two shapes correspond to the two halves of knot's split view:
//!
//! - `preview`: many small `TextWidget`s in a column — the markdown preview,
//!   one widget per block.
//! - `editor`: one multi-line `Input` holding the whole document.
//!
//! Both are measured at several document sizes, because the question is not
//! "how slow is it" but "what does it scale with".

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use shroud_core::Theme;
use shroud_text::TextEngine;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input, TextWidget};

/// A paragraph of prose, close enough to real note text that shaping and
/// wrapping do representative work.
const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog, and \
then keeps going for long enough that this line has to wrap at least once \
inside a six-hundred pixel column.";

const VIEWPORT: (f32, f32) = (900.0, 700.0);

/// Column of `blocks` text widgets — the markdown preview's shape.
fn preview_tree(blocks: usize) -> WidgetTree {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(600.0).height(VIEWPORT.1));
    for i in 0..blocks {
        tree.add_child(root, TextWidget::new(format!("{i}. {PARAGRAPH}")));
    }
    tree
}

/// Same node count, near-zero text. Subtracting this from `preview_tree` at
/// the same block count splits the per-frame cost into the part that scales
/// with *text* (shaping / measuring) and the part that scales with *nodes*
/// (the dirty-marking, node-map and taffy work the pass does regardless).
fn preview_tree_short(blocks: usize) -> WidgetTree {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(600.0).height(VIEWPORT.1));
    for _ in 0..blocks {
        tree.add_child(root, TextWidget::new("x"));
    }
    tree
}

/// One multi-line `Input` holding `blocks` paragraphs — the editor's shape.
fn editor_tree(blocks: usize) -> WidgetTree {
    let body: String = (0..blocks)
        .map(|i| format!("{i}. {PARAGRAPH}\n\n"))
        .collect();
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(600.0).height(VIEWPORT.1));
    tree.add_child(
        root,
        Input::new()
            .multiline()
            .with_value(body)
            .height(VIEWPORT.1 - 40.0),
    );
    tree
}

/// One layout pass, exactly as the event loop runs it every frame.
///
/// The engine is reused across iterations on purpose: a fresh one would
/// measure cold shaping, and the frames under investigation were *warm* —
/// every shape was a cache hit and layout was still 19 ms.
fn layout_once(tree: &mut WidgetTree, engine: &mut TextEngine, theme: &Theme) {
    tree.compute_layout_with_measure(VIEWPORT.0, VIEWPORT.1, engine, theme);
}

fn bench_preview(c: &mut Criterion) {
    let theme = Theme::default();
    let mut group = c.benchmark_group("layout/preview_blocks");
    for blocks in [16usize, 64, 256] {
        let mut tree = preview_tree(blocks);
        let mut engine = TextEngine::new();
        // Warm the shape cache so the measurement matches the steady state.
        layout_once(&mut tree, &mut engine, &theme);
        group.bench_with_input(BenchmarkId::from_parameter(blocks), &blocks, |b, _| {
            b.iter(|| layout_once(black_box(&mut tree), &mut engine, &theme));
        });
    }
    group.finish();
}

fn bench_preview_short(c: &mut Criterion) {
    let theme = Theme::default();
    let mut group = c.benchmark_group("layout/preview_blocks_short");
    for blocks in [16usize, 64, 256] {
        let mut tree = preview_tree_short(blocks);
        let mut engine = TextEngine::new();
        layout_once(&mut tree, &mut engine, &theme);
        group.bench_with_input(BenchmarkId::from_parameter(blocks), &blocks, |b, _| {
            b.iter(|| layout_once(black_box(&mut tree), &mut engine, &theme));
        });
    }
    group.finish();
}

fn bench_editor(c: &mut Criterion) {
    let theme = Theme::default();
    let mut group = c.benchmark_group("layout/editor_paragraphs");
    for blocks in [16usize, 64, 256] {
        let mut tree = editor_tree(blocks);
        let mut engine = TextEngine::new();
        layout_once(&mut tree, &mut engine, &theme);
        group.bench_with_input(BenchmarkId::from_parameter(blocks), &blocks, |b, _| {
            b.iter(|| layout_once(black_box(&mut tree), &mut engine, &theme));
        });
    }
    group.finish();
}

/// What one `measure` costs when the shape cache already holds the answer.
///
/// `Text::measure` only reads `width` / `height` off the result, but
/// `shape_text_attrs` returns `ShapedText` **by value** — so a cache *hit*
/// still clones the whole glyph vector. This isolates that clone from the
/// rest of the layout pass.
fn bench_shape_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("shape_cache_hit");
    for paragraphs in [1usize, 4, 16] {
        let text: String = (0..paragraphs).map(|_| format!("{PARAGRAPH} ")).collect();
        let mut engine = TextEngine::new();
        // Prime the cache; every timed call below is a hit.
        let warm = engine.shape_text(&text, 14.0, 20.0, Some(600.0));
        let glyphs = warm.glyphs.len();
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{paragraphs}p_{glyphs}glyphs")),
            &text,
            |b, text| {
                b.iter(|| black_box(engine.shape_text(black_box(text), 14.0, 20.0, Some(600.0))));
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_preview,
    bench_preview_short,
    bench_editor,
    bench_shape_cache_hit
);
criterion_main!(benches);
