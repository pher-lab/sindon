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

/// A document of `paragraphs` lines, with `extra` appended to line
/// `edited_line` — two calls differing only in `extra` give a pair of documents
/// that differ by a single keystroke on one line.
fn keystroke_doc(paragraphs: usize, edited_line: usize, extra: &str) -> String {
    (0..paragraphs)
        .map(|i| {
            if i == edited_line {
                format!("{i}. {PARAGRAPH}{extra}\n")
            } else {
                format!("{i}. {PARAGRAPH}\n")
            }
        })
        .collect()
}

/// What one keystroke costs a focused `Input`.
///
/// The focused editing path can't use the shape cache — every keystroke's
/// value is a unique key — so the question is what it re-shapes. Two arms,
/// measured in the same run so they're directly comparable:
///
/// - `incremental`: `shape_edit_plain`, which keeps the engine's buffer between
///   calls and rewrites only the line that changed.
/// - `full_rebuild`: a fresh buffer over the whole value, which is what the
///   editing path did before — and what `shapes=1` costing 10 ms in the
///   `SHROUD_PERF` log was measuring.
///
/// The two arms shape the same documents, so the gap between them is the cost
/// of re-shaping the untouched remainder of the note.
fn bench_edit_keystroke(c: &mut Criterion) {
    let attrs = shroud_text::TextAttrs::default();
    let (fs, lh, wrap) = (14.0, 20.0, Some(600.0));

    let mut group = c.benchmark_group("edit_keystroke");
    for paragraphs in [16usize, 64, 256] {
        let a = keystroke_doc(paragraphs, paragraphs / 2, "");
        let b = keystroke_doc(paragraphs, paragraphs / 2, "!");

        let mut engine = TextEngine::new();
        // Warm the buffer, so the timed calls are the steady typing state.
        let _ = engine.shape_edit_plain(&a, fs, lh, wrap, &attrs);
        let mut flip = false;
        group.bench_with_input(
            BenchmarkId::new("incremental", paragraphs),
            &paragraphs,
            |bencher, _| {
                bencher.iter(|| {
                    flip = !flip;
                    let text = if flip { &b } else { &a };
                    black_box(engine.shape_edit_plain(black_box(text), fs, lh, wrap, &attrs));
                });
            },
        );

        let mut engine = TextEngine::new();
        let mut flip = false;
        group.bench_with_input(
            BenchmarkId::new("full_rebuild", paragraphs),
            &paragraphs,
            |bencher, _| {
                bencher.iter(|| {
                    flip = !flip;
                    let text = if flip { &b } else { &a };
                    black_box(engine.shape_text_uncached(black_box(text), fs, lh, wrap));
                });
            },
        );

        // The same two documents through the *rich* path — what a field with a
        // live highlighter (knot's editor) actually costs per keystroke.
        let (spans_a, spans_b) = (highlight_tiling(&a), highlight_tiling(&b));

        let mut engine = TextEngine::new();
        let _ = engine.shape_edit_rich(&spans_a, fs, lh, wrap);
        let mut flip = false;
        group.bench_with_input(
            BenchmarkId::new("incremental_rich", paragraphs),
            &paragraphs,
            |bencher, _| {
                bencher.iter(|| {
                    flip = !flip;
                    let spans = if flip { &spans_b } else { &spans_a };
                    black_box(engine.shape_edit_rich(black_box(spans), fs, lh, wrap));
                });
            },
        );

        // The frame that changes nothing — ~91% of a real editing session.
        // Line reuse alone still walked the whole document here to discover
        // that; the input digest skips the walk entirely.
        let mut engine = TextEngine::new();
        let _ = engine.shape_edit_rich(&spans_a, fs, lh, wrap);
        group.bench_with_input(
            BenchmarkId::new("idle_repaint_rich", paragraphs),
            &paragraphs,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(engine.shape_edit_rich(black_box(&spans_a), fs, lh, wrap));
                });
            },
        );

        let mut engine = TextEngine::new();
        let mut flip = false;
        group.bench_with_input(
            BenchmarkId::new("full_rebuild_rich", paragraphs),
            &paragraphs,
            |bencher, _| {
                bencher.iter(|| {
                    flip = !flip;
                    let spans = if flip { &spans_b } else { &spans_a };
                    // Clearing first forces the miss, which is the whole-buffer
                    // rebuild the rich editing path used to do every frame.
                    engine.clear_shape_cache();
                    black_box(engine.shape_rich(black_box(spans), fs, lh, wrap));
                });
            },
        );
    }
    group.finish();
}

/// Tile a document into color-only spans the way a live highlighter does: one
/// span per whitespace-delimited word, every third one colored.
fn highlight_tiling(text: &str) -> Vec<shroud_text::TextSpan> {
    let accent = shroud_core::Color::rgb(0.85, 0.4, 0.2);
    text.split_inclusive(' ')
        .enumerate()
        .map(|(i, chunk)| {
            let span = shroud_text::TextSpan::new(chunk);
            if i % 3 == 0 { span.color(accent) } else { span }
        })
        .collect()
}

criterion_group!(
    benches,
    bench_preview,
    bench_preview_short,
    bench_editor,
    bench_shape_cache_hit,
    bench_edit_keystroke
);
criterion_main!(benches);
