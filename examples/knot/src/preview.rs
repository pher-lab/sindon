//! Markdown preview — render a note body into a shroud widget tree.
//!
//! This is the B-2 "app piece": the framework grew everything the spike
//! needed (rich inline text + `flex_wrap` + `flex_basis` + ScrollView auto
//! content height), so the renderer here is just the pulldown-cmark → widget
//! mapping, owned by the app rather than the framework.
//!
//! Lifted from `examples/markdown_demo/src/markdown.rs` and made
//! theme-aware: where the demo hardcodes a dark palette, this reads Knot's
//! live theme tokens (via [`crate::settings`]) so the preview tracks a
//! Light/Dark/System swap in lockstep with the rest of the app.
//!
//! Colors come in two flavors:
//!   * **Block-level** text and panel fills use `Reactive<Color>` derived
//!     from `current_theme()`, so a theme swap while the preview is open
//!     repaints them without a rebuild.
//!   * **Inline span** colors (inline `code`, links) are baked statically at
//!     build time — `TextSpan::color` takes a plain `Color` because spans are
//!     shaped together. These only refresh when the preview subtree is rebuilt
//!     (i.e. on the next edit⇄preview toggle), which is acceptable: a theme
//!     swap mid-preview leaves just the inline-code / link tint a beat stale.
//!
//! GFM tables and task lists are rendered (see [`options`]). Still out of
//! scope: strikethrough, syntax highlighting, images, wikilinks, and
//! link-click handling. Strikethrough and clickable links each need framework
//! support that doesn't exist yet — a text-decoration attribute on glyphs, and
//! per-span click targets respectively (`TextWidget::rich` shapes a whole line
//! as one widget, so there's nowhere to hang a per-link handler) — so they're
//! deferred rather than rendered half-right.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use shroud::core::Color;
use shroud::reactive::Reactive;
use shroud::text::TextSpan;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, TextWidget};

use crate::settings;

// ── Reactive block-level theme colors ───────────────────────────────────────
// Each re-reads `current_theme()` on every paint, so block text and panel
// fills follow a live theme swap.

fn body_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.on_surface)
}
fn muted_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.on_surface_variant)
}
fn accent_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.primary)
}
fn code_bg_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.surface_variant)
}
/// Subtle line color for table dividers — the same token inputs use for their
/// resting border, so dividers read as structure rather than text.
fn border_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.input_border)
}

/// Static accent used for inline `code` and link spans. Resolved once per
/// block at build time (see the module note on why spans can't be reactive).
fn inline_accent() -> Color {
    settings::current_theme().colors.primary
}

#[derive(Clone, Copy, PartialEq)]
enum InlineStyle {
    Plain,
    Bold,
    Italic,
    Code,
    Link,
}

struct InlineRun {
    text: String,
    style: InlineStyle,
}

/// Line box for a heading at `size` px. Headings bump `font_size` without a
/// matching `line_height`, which would otherwise leave the tall glyph in the
/// default ~22px body line box and overflow it — clipping the top of the
/// first block ("the heading sinks into the top edge"). ~1.3× mirrors the
/// theme's own heading ratio (36/28).
fn heading_line_height(size: f32) -> f32 {
    (size * 1.3).round()
}

/// Whether `runs` can take the single-`TextWidget` fast path. Only when there
/// is nothing to style: an empty block, or runs that are *all* `Plain`.
///
/// A single non-plain run (e.g. a paragraph that is just `**bold**`) must NOT
/// take this path — the fast path emits a plain `TextWidget` and drops the
/// run's style, so a lone bold/italic/code run would render unstyled. Those
/// go the rich path so the attribute survives.
fn is_fast_path(runs: &[InlineRun]) -> bool {
    runs.is_empty() || runs.iter().all(|r| r.style == InlineStyle::Plain)
}

/// GFM extensions the preview parser understands. Tables and task lists are
/// rendered; strikethrough is deliberately left off so `~~text~~` stays
/// literal rather than collapsing to plain text we can't visually strike.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS
}

/// Render `source` as markdown into `parent`. `parent` must be a column-flex
/// container; one block is appended per top-level markdown block. An empty /
/// whitespace-only source renders a single muted placeholder so the preview
/// pane never looks broken on a brand-new note.
pub fn render(tree: &mut WidgetTree, parent: usize, source: &str) {
    if source.trim().is_empty() {
        tree.add_child(
            parent,
            TextWidget::new("Nothing to preview yet.").color(muted_color()),
        );
        return;
    }

    let parser = Parser::new_ext(source, options());
    let events: Vec<Event> = parser.collect();
    let mut i = 0;
    while i < events.len() {
        i = render_block(tree, parent, &events, i);
    }
}

fn render_block(tree: &mut WidgetTree, parent: usize, events: &[Event], start: usize) -> usize {
    match &events[start] {
        Event::Start(Tag::Heading { level, .. }) => {
            let (runs, end) =
                collect_inline(events, start + 1, |t| matches!(t, TagEnd::Heading(_)));
            let size = match level {
                HeadingLevel::H1 => 32.0,
                HeadingLevel::H2 => 26.0,
                HeadingLevel::H3 => 22.0,
                _ => 18.0,
            };
            emit_inline_block(tree, parent, runs, Some(size), false);
            end + 1
        }
        Event::Start(Tag::Paragraph) => {
            let (runs, end) = collect_inline(events, start + 1, |t| matches!(t, TagEnd::Paragraph));
            emit_inline_block(tree, parent, runs, None, false);
            end + 1
        }
        Event::Start(Tag::BlockQuote) => {
            // Vertical bar + indented column. The inner column nests recursive
            // block rendering (a blockquote can contain paragraphs, lists…).
            //
            // `flex_basis(0).grow(1)` on the body is the CSS-idiomatic `flex:
            // 1 1 0`: body starts at zero main-axis width and takes the row's
            // leftover space without first expanding to its text's natural
            // unwrapped width. Without it the body either collapses to width 0
            // (no basis) or overflows to natural text width (with `grow` but
            // no zero basis), and the bar ends up squeezed or invisibly short.
            let row = tree.add_child(parent, Container::row().gap(12.0));
            tree.add_child(
                row,
                Container::column().width(4.0).background(muted_color()),
            );
            let body = tree.add_child(row, Container::column().gap(8.0).flex_basis(0.0).grow(1.0));
            let mut i = start + 1;
            let mut depth = 1;
            let inner_start = i;
            while i < events.len() {
                match &events[i] {
                    Event::Start(Tag::BlockQuote) => depth += 1,
                    Event::End(TagEnd::BlockQuote) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let inner_end = i;
            let mut j = inner_start;
            while j < inner_end {
                j = render_block(tree, body, events, j);
            }
            inner_end + 1
        }
        Event::Start(Tag::CodeBlock(kind)) => {
            let _ = kind; // language ignored (no syntect in scope)
            let mut buf = String::new();
            let mut i = start + 1;
            while i < events.len() {
                match &events[i] {
                    Event::Text(t) => buf.push_str(t),
                    Event::End(TagEnd::CodeBlock) => break,
                    _ => {}
                }
                i += 1;
            }
            // Trailing newline is conventional in pulldown-cmark output.
            let buf = buf.trim_end_matches('\n').to_string();
            let block = tree.add_child(
                parent,
                Container::column()
                    .padding(12.0)
                    .background(code_bg_color())
                    .radius(6.0),
            );
            // One TextWidget per code line. A single TextWidget would also work
            // (cosmic-text honors '\n') but per-line is closer to how a real
            // highlighter would emit decorated spans.
            for line in buf.split('\n') {
                tree.add_child(
                    block,
                    TextWidget::new(if line.is_empty() { " " } else { line })
                        .color(accent_color())
                        .monospace(),
                );
            }
            i + 1
        }
        Event::Start(Tag::List(first_num)) => {
            let ordered_start = *first_num;
            let list = tree.add_child(parent, Container::column().gap(4.0));
            let mut i = start + 1;
            let mut counter = ordered_start.unwrap_or(0);
            while i < events.len() {
                match &events[i] {
                    Event::Start(Tag::Item) => {
                        // A task list emits a `TaskListMarker(checked)` as the
                        // first event inside the item; when present, swap the
                        // bullet/number for an ASCII checkbox and start the body
                        // after the marker so it isn't rendered as text.
                        //
                        // ASCII `[x]`/`[ ]` rather than the ballot-box glyphs
                        // (☑ U+2611 / ☐ U+2610): U+2611 isn't in the primary
                        // font here and falls back to an oversized ■, so the two
                        // states render at mismatched sizes. ASCII stays one
                        // font, one size, everywhere.
                        let (marker, body_start) = match events.get(i + 1) {
                            Some(Event::TaskListMarker(checked)) => {
                                let glyph = if *checked { "[x]" } else { "[ ]" };
                                (glyph.to_string(), i + 2)
                            }
                            _ if ordered_start.is_some() => {
                                let m = format!("{}.", counter);
                                counter += 1;
                                (m, i + 1)
                            }
                            _ => ("\u{2022}".to_string(), i + 1),
                        };
                        let row = tree.add_child(list, Container::row().gap(8.0));
                        tree.add_child(row, TextWidget::new(marker).color(muted_color()));
                        // Same `flex: 1 1 0` shape as the blockquote body —
                        // long list items should wrap to the row's leftover
                        // width, not push the marker / overflow horizontally.
                        let item_body = tree
                            .add_child(row, Container::column().gap(4.0).flex_basis(0.0).grow(1.0));
                        i = render_list_item(tree, item_body, events, body_start);
                    }
                    Event::End(TagEnd::List(_)) => break,
                    _ => i += 1,
                }
            }
            i + 1
        }
        Event::Start(Tag::Table(aligns)) => render_table(tree, parent, events, start, aligns),
        // Stray text outside a block (rare; defensive).
        Event::Text(t) => {
            tree.add_child(parent, TextWidget::new(t.to_string()).color(body_color()));
            start + 1
        }
        _ => start + 1,
    }
}

/// Render the inside of a `<li>` — typically one paragraph, sometimes nested
/// blocks (sublists). Returns the index *after* the closing `TagEnd::Item`.
fn render_list_item(tree: &mut WidgetTree, parent: usize, events: &[Event], start: usize) -> usize {
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(TagEnd::Item) => return i + 1,
            // pulldown-cmark wraps loose item text in a paragraph; tight lists
            // emit raw inline events directly inside Item.
            Event::Start(Tag::Paragraph) => {
                let (runs, end) = collect_inline(events, i + 1, |t| matches!(t, TagEnd::Paragraph));
                emit_inline_block(tree, parent, runs, None, false);
                i = end + 1;
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Link { .. }) => {
                let (runs, end) = collect_inline(events, i, |t| matches!(t, TagEnd::Item));
                emit_inline_block(tree, parent, runs, None, false);
                i = end;
            }
            Event::Start(Tag::List(_))
            | Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::BlockQuote)
            | Event::Start(Tag::Table(_)) => {
                i = render_block(tree, parent, events, i);
            }
            _ => i += 1,
        }
    }
    i
}

/// Render a GFM table into `parent`. Returns the index after `TagEnd::Table`.
///
/// Layout is a column of rows; each row is a flex row whose cells share the
/// available width equally (`flex: 1 1 0`) and wrap their text — there is no
/// horizontal scroll (ScrollView is vertical-only), so a wide table reflows
/// rather than overflowing. A thin divider separates the header from the body
/// and each body row from the next.
fn render_table(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    start: usize,
    aligns: &[Alignment],
) -> usize {
    let table = tree.add_child(parent, Container::column());
    let mut i = start + 1;
    let mut first_body_row = true;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::TableHead) => {
                i = render_table_row(tree, table, events, i + 1, aligns, true);
                add_divider(tree, table);
            }
            Event::Start(Tag::TableRow) => {
                // Divider goes *between* body rows; the header already laid one
                // down, so the first body row skips its leading divider.
                if !first_body_row {
                    add_divider(tree, table);
                }
                first_body_row = false;
                i = render_table_row(tree, table, events, i + 1, aligns, false);
            }
            Event::End(TagEnd::Table) => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// Render one table row (header or body) into `table`. `start` is the index of
/// the first event after the `TableHead`/`TableRow` start tag; returns the
/// index after the row's closing tag. Header cells render bold.
fn render_table_row(
    tree: &mut WidgetTree,
    table: usize,
    events: &[Event],
    start: usize,
    aligns: &[Alignment],
    header: bool,
) -> usize {
    let row = tree.add_child(table, Container::row());
    let mut col = 0usize;
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::TableCell) => {
                let mut cell = Container::column().flex_basis(0.0).grow(1.0).padding(8.0);
                // Only center is expressible (Container has no align-end), so
                // left and right both fall back to the leading edge.
                if matches!(aligns.get(col), Some(Alignment::Center)) {
                    cell = cell.align_center();
                }
                let cell_idx = tree.add_child(row, cell);
                let (runs, end) = collect_inline(events, i + 1, |t| matches!(t, TagEnd::TableCell));
                emit_inline_block(tree, cell_idx, runs, None, header);
                i = end + 1;
                col += 1;
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// A 1px full-width line separating table rows.
fn add_divider(tree: &mut WidgetTree, parent: usize) {
    tree.add_child(
        parent,
        Container::row()
            .height(1.0)
            .width_full()
            .background(border_color()),
    );
}

/// Walk inline events, splitting them into typed runs. Stops *at* the index of
/// the matching block-end tag (does not consume it). The caller is responsible
/// for the `+1` to step over the end tag.
fn collect_inline(
    events: &[Event],
    start: usize,
    is_end: impl Fn(&TagEnd) -> bool,
) -> (Vec<InlineRun>, usize) {
    let mut runs: Vec<InlineRun> = Vec::new();
    let mut style_stack: Vec<InlineStyle> = vec![InlineStyle::Plain];
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(t) if is_end(t) => return (runs, i),
            Event::Text(s) => {
                let style = *style_stack.last().unwrap();
                runs.push(InlineRun {
                    text: s.to_string(),
                    style,
                });
            }
            Event::Code(s) => runs.push(InlineRun {
                text: s.to_string(),
                style: InlineStyle::Code,
            }),
            Event::SoftBreak | Event::HardBreak => runs.push(InlineRun {
                text: " ".into(),
                style: InlineStyle::Plain,
            }),
            Event::Start(Tag::Strong) => style_stack.push(InlineStyle::Bold),
            Event::Start(Tag::Emphasis) => style_stack.push(InlineStyle::Italic),
            Event::Start(Tag::Link { .. }) => style_stack.push(InlineStyle::Link),
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link) => {
                style_stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
    (runs, i)
}

/// Emit one paragraph- or heading-shaped block from `runs`.
///
/// - **0 or 1 plain runs**: emit a single `TextWidget::new` (cleanest path,
///   no rich-text overhead).
/// - **Mixed inline styling**: emit a single `TextWidget::rich(Vec<TextSpan>)`
///   so the shaper sees all spans as one logical line and wraps either at span
///   boundaries *or* inside an attributed span.
fn emit_inline_block(
    tree: &mut WidgetTree,
    parent: usize,
    runs: Vec<InlineRun>,
    font_size: Option<f32>,
    bold: bool,
) {
    // Headings imply bold via `font_size`; `bold` forces it for non-heading
    // blocks that still want weight (table header cells).
    let want_bold = bold || font_size.is_some();
    if is_fast_path(&runs) {
        let text: String = runs.into_iter().map(|r| r.text).collect();
        let mut w = TextWidget::new(text).color(body_color());
        if let Some(s) = font_size {
            // Headings: bigger glyph with a line box tall enough for it (see
            // `heading_line_height`). Body paragraphs keep the default size.
            w = w.font_size(s).line_height(heading_line_height(s));
        }
        if want_bold {
            w = w.bold();
        }
        tree.add_child(parent, w);
        return;
    }

    // Multi-run path: one TextWidget::rich, one span per run. The widget-level
    // `.color(body_color())` is the reactive fallback for spans that don't
    // override color (Plain/Bold/Italic); Code/Link set their own static
    // per-span color, which wins for those glyphs only.
    let accent = inline_accent();
    let spans: Vec<TextSpan> = runs
        .into_iter()
        .map(|run| {
            let mut span = TextSpan::new(run.text);
            match run.style {
                InlineStyle::Plain => {}
                InlineStyle::Bold => span = span.bold(),
                InlineStyle::Italic => span = span.italic(),
                InlineStyle::Code => span = span.monospace().color(accent),
                InlineStyle::Link => span = span.color(accent),
            }
            if want_bold {
                // Mixed-run headings / table headers: spans don't inherit the
                // wrapping widget's `.bold()`, so set weight per span too.
                span = span.bold();
            }
            span
        })
        .collect();

    let mut w = TextWidget::rich(spans).color(body_color());
    if let Some(s) = font_size {
        w = w.font_size(s).line_height(heading_line_height(s));
    }
    tree.add_child(parent, w);
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroud::core::Theme;
    use shroud::text::TextEngine;
    use shroud::widgets::Container;

    /// Build `source` into a content column and lay it out, returning the tree
    /// plus the content-column index. Exercises the full parse → widget →
    /// measure path the live preview runs, so callers can assert on either the
    /// laid-out geometry or the widget structure.
    fn build(source: &str) -> (WidgetTree, usize) {
        let mut tree = WidgetTree::new();
        let root = tree.set_root(Container::column().width(600.0).height(800.0).padding(16.0));
        let col = tree.add_child(root, Container::column().width_full().gap(12.0));
        render(&mut tree, col, source);

        let mut engine = TextEngine::new();
        let theme = Theme::dark();
        tree.compute_layout_with_measure(600.0, 800.0, &mut engine, &theme);
        (tree, col)
    }

    /// The laid-out height of `source`'s content column.
    fn rendered_height(source: &str) -> f32 {
        let (tree, col) = build(source);
        tree.layout_rect(col).size.height
    }

    #[test]
    fn empty_source_renders_placeholder_not_nothing() {
        // A brand-new (empty) note must still produce a visible block so the
        // preview pane doesn't look broken.
        assert!(rendered_height("").size_is_positive());
        assert!(rendered_height("   \n  ").size_is_positive());
    }

    #[test]
    fn every_block_type_renders_without_panicking() {
        // One sample touching heading / paragraph / rich inline / list /
        // blockquote / code block. The assertion is deliberately loose (just
        // "produced real height"): the point is that none of the pulldown
        // event shapes hit an unhandled path or a measure panic.
        let sample = "\
# Title

A paragraph with **bold**, *italic*, `code`, and a [link](knot://x).

- one
- two

- [ ] todo
- [x] done

| col a | col b |
|-------|-------|
| 1     | 2     |

> quoted line

```
fn main() {}
```
";
        let h = rendered_height(sample);
        assert!(h > 100.0, "multi-block sample should be tall, got {h}");
    }

    #[test]
    fn task_list_items_render_one_row_each() {
        // Each item (checked, unchecked, or a plain bullet mixed in) is its own
        // marker+body row — the task markers don't leak into the body text or
        // collapse two items into one.
        let (tree, col) = build("- [ ] todo\n- [x] done\n- plain bullet");
        let blocks = tree.children(col);
        assert_eq!(blocks.len(), 1, "a single list is one top-level block");
        let items = tree.children(blocks[0]);
        assert_eq!(items.len(), 3, "three list items render three rows");
        for row in items {
            assert_eq!(tree.children(row).len(), 2, "each row is marker + body");
        }
    }

    #[test]
    fn table_renders_header_dividers_and_body_rows() {
        // header + divider + body row + divider + body row = five children, and
        // each rendered row carries one cell per column.
        let (tree, col) = build("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |");
        let blocks = tree.children(col);
        assert_eq!(blocks.len(), 1, "a table is one top-level block");
        let rows = tree.children(blocks[0]);
        assert_eq!(rows.len(), 5, "header, divider, row, divider, row");
        assert_eq!(tree.children(rows[0]).len(), 2, "header has two cells");
        assert_eq!(tree.children(rows[1]).len(), 0, "divider has no cells");
        assert_eq!(tree.children(rows[2]).len(), 2, "body row has two cells");
        assert_eq!(tree.children(rows[3]).len(), 0, "divider has no cells");
        assert_eq!(tree.children(rows[4]).len(), 2, "body row has two cells");
    }

    #[test]
    fn lone_styled_run_does_not_take_the_plain_fast_path() {
        // Regression for the "bold doesn't apply unless followed by text" bug:
        // a block that is a single styled run must go the rich path so its
        // attribute survives, while all-plain blocks still take the cheap path.
        let bold_only = vec![InlineRun {
            text: "bold".into(),
            style: InlineStyle::Bold,
        }];
        assert!(
            !is_fast_path(&bold_only),
            "a lone bold run must render rich, not as plain text"
        );

        let plain_only = vec![InlineRun {
            text: "hello".into(),
            style: InlineStyle::Plain,
        }];
        assert!(
            is_fast_path(&plain_only),
            "all-plain stays on the fast path"
        );
        assert!(is_fast_path(&[]), "empty stays on the fast path");

        let mixed = vec![
            InlineRun {
                text: "a ".into(),
                style: InlineStyle::Plain,
            },
            InlineRun {
                text: "b".into(),
                style: InlineStyle::Code,
            },
        ];
        assert!(!is_fast_path(&mixed), "a styled run anywhere forces rich");
    }

    #[test]
    fn top_heading_line_box_contains_its_glyph() {
        // Regression for the "heading sinks into the top edge" bug: an H1's
        // rendered height must be at least its 32px font size, so the glyph
        // isn't clipping out the top of a too-short default line box.
        let h = rendered_height("# Heading");
        assert!(
            h >= 32.0,
            "H1 line box must fit a 32px glyph, got {h}px tall"
        );
    }

    #[test]
    fn more_content_is_taller() {
        // Sanity that the renderer scales with content rather than emitting a
        // fixed-size stub — a three-paragraph note must out-measure a one-liner.
        let short = rendered_height("Just one line.");
        let long = rendered_height("Para one.\n\nPara two.\n\nPara three.\n\nPara four.");
        assert!(
            long > short,
            "more paragraphs must be taller: {long} vs {short}"
        );
    }

    // Tiny readability shim so the empty-source asserts read intention-first.
    trait PositiveSize {
        fn size_is_positive(&self) -> bool;
    }
    impl PositiveSize for f32 {
        fn size_is_positive(&self) -> bool {
            *self > 0.0
        }
    }
}
