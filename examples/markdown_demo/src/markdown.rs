//! Markdown → shroud widget tree renderer (B-2 lite spike).
//!
//! This module is intentionally written *with shroud's current primitives
//! only* — no new framework APIs. The point of the spike is to drive the
//! port from pulldown-cmark events to widgets and surface, by failure, the
//! framework gaps that B-2 would need to fix.
//!
//! See `memory/progress_b2_spike.md` for the gap inventory this surfaced.
//!
//! Out of scope on purpose: GFM tables, task lists, strikethrough, syntax
//! highlighting, images, wikilinks, link click handling.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use shroud::core::Color;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, TextWidget};

const COLOR_BODY: Color = Color {
    r: 0.85,
    g: 0.85,
    b: 0.88,
    a: 1.0,
};
const COLOR_MUTED: Color = Color {
    r: 0.55,
    g: 0.55,
    b: 0.60,
    a: 1.0,
};
const COLOR_BOLD: Color = Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const COLOR_ITALIC: Color = Color {
    r: 0.80,
    g: 0.85,
    b: 1.0,
    a: 1.0,
};
const COLOR_CODE_FG: Color = Color {
    r: 1.0,
    g: 0.85,
    b: 0.60,
    a: 1.0,
};
const COLOR_LINK: Color = Color {
    r: 0.55,
    g: 0.75,
    b: 1.0,
    a: 1.0,
};
const COLOR_CODE_BG: Color = Color {
    r: 0.08,
    g: 0.08,
    b: 0.10,
    a: 1.0,
};
const COLOR_QUOTE_BAR: Color = Color {
    r: 0.40,
    g: 0.40,
    b: 0.48,
    a: 1.0,
};

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

/// Render `source` as markdown into `parent`. Parent must be a column-flex
/// container; the renderer appends one block per top-level markdown block.
pub fn render(tree: &mut WidgetTree, parent: usize, source: &str) {
    let parser = Parser::new(source);
    let events: Vec<Event> = parser.collect();
    let mut i = 0;
    while i < events.len() {
        i = render_block(tree, parent, &events, i);
    }
}

fn render_block(tree: &mut WidgetTree, parent: usize, events: &[Event], start: usize) -> usize {
    match &events[start] {
        Event::Start(Tag::Heading { level, .. }) => {
            let (runs, end) = collect_inline(events, start + 1, |t| {
                matches!(t, TagEnd::Heading(_))
            });
            let size = match level {
                HeadingLevel::H1 => 32.0,
                HeadingLevel::H2 => 26.0,
                HeadingLevel::H3 => 22.0,
                _ => 18.0,
            };
            emit_inline_block(tree, parent, runs, Some(size));
            end + 1
        }
        Event::Start(Tag::Paragraph) => {
            let (runs, end) = collect_inline(events, start + 1, |t| {
                matches!(t, TagEnd::Paragraph)
            });
            emit_inline_block(tree, parent, runs, None);
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
            tree.add_child(row, Container::column().width(4.0).background(COLOR_QUOTE_BAR));
            let body = tree.add_child(
                row,
                Container::column().gap(8.0).flex_basis(0.0).grow(1.0),
            );
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
            let _ = kind; // language ignored in spike (no syntect)
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
                    .background(COLOR_CODE_BG)
                    .radius(6.0),
            );
            // One TextWidget per code line. Single TextWidget would also work
            // (cosmic-text honors '\n') but per-line is closer to how a real
            // highlighter would emit decorated spans.
            for line in buf.split('\n') {
                tree.add_child(
                    block,
                    TextWidget::new(if line.is_empty() { " " } else { line })
                        .color(COLOR_CODE_FG),
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
                        let row = tree.add_child(list, Container::row().gap(8.0));
                        let marker = if ordered_start.is_some() {
                            let m = format!("{}.", counter);
                            counter += 1;
                            m
                        } else {
                            "\u{2022}".into()
                        };
                        tree.add_child(row, TextWidget::new(marker).color(COLOR_MUTED));
                        // Same `flex: 1 1 0` shape as the blockquote body —
                        // long list items should wrap to the row's leftover
                        // width, not push the marker / overflow horizontally.
                        let item_body = tree.add_child(
                            row,
                            Container::column().gap(4.0).flex_basis(0.0).grow(1.0),
                        );
                        i = render_list_item(tree, item_body, events, i + 1);
                    }
                    Event::End(TagEnd::List(_)) => break,
                    _ => i += 1,
                }
            }
            i + 1
        }
        // Stray text outside a block (rare; defensive).
        Event::Text(t) => {
            tree.add_child(parent, TextWidget::new(t.to_string()).color(COLOR_BODY));
            start + 1
        }
        _ => start + 1,
    }
}

/// Render the inside of a `<li>` — typically one paragraph, sometimes
/// nested blocks (sublists). Returns the index *after* the closing
/// `TagEnd::Item`.
fn render_list_item(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    start: usize,
) -> usize {
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(TagEnd::Item) => return i + 1,
            // pulldown-cmark wraps loose item text in a paragraph; tight
            // lists emit raw inline events directly inside Item.
            Event::Start(Tag::Paragraph) => {
                let (runs, end) = collect_inline(events, i + 1, |t| {
                    matches!(t, TagEnd::Paragraph)
                });
                emit_inline_block(tree, parent, runs, None);
                i = end + 1;
            }
            Event::Text(_) | Event::Code(_) | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Link { .. }) => {
                let (runs, end) = collect_inline(events, i, |t| matches!(t, TagEnd::Item));
                emit_inline_block(tree, parent, runs, None);
                i = end;
            }
            Event::Start(Tag::List(_)) | Event::Start(Tag::CodeBlock(_)) | Event::Start(Tag::BlockQuote) => {
                i = render_block(tree, parent, events, i);
            }
            _ => i += 1,
        }
    }
    i
}

/// Walk inline events, splitting them into typed runs. Stops *at* the index
/// of the matching block-end tag (does not consume it). The caller is
/// responsible for the `+1` to step over the end tag.
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
                runs.push(InlineRun { text: s.to_string(), style });
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
/// Strategy:
/// - **0 or 1 runs**: emit a single `TextWidget`, which wraps natively. This
///   is the common case (plain paragraphs) and the only one that lays out
///   correctly with current shroud.
/// - **Multiple runs (mixed inline styling)**: emit a `Container::row()` of
///   per-run `TextWidget`s. This is where the spike *fails by design*:
///   - `Container::row()` has no `flex_wrap`, so a long mixed-style
///     paragraph overflows the parent's width instead of breaking.
///   - shroud's `TextWidget` has no font-weight / font-style / font-family
///     knob, so bold/italic/code visually collapse to plain text plus a
///     color tweak. The user can't tell a bold word from a tinted word.
fn emit_inline_block(
    tree: &mut WidgetTree,
    parent: usize,
    runs: Vec<InlineRun>,
    font_size: Option<f32>,
) {
    let single = matches!(runs.len(), 0 | 1)
        || runs.iter().all(|r| r.style == InlineStyle::Plain);
    if single {
        let text: String = runs.into_iter().map(|r| r.text).collect();
        let mut w = TextWidget::new(text).color(COLOR_BODY);
        if let Some(s) = font_size {
            w = w.font_size(s);
        }
        tree.add_child(parent, w);
        return;
    }

    // Multi-run path — known to be broken (no flex_wrap, no font weight),
    // kept so the spike makes the gap visible in real pixels.
    let row = tree.add_child(parent, Container::row().gap(0.0));
    for run in runs {
        let color = match run.style {
            InlineStyle::Plain => COLOR_BODY,
            InlineStyle::Bold => COLOR_BOLD,
            InlineStyle::Italic => COLOR_ITALIC,
            InlineStyle::Code => COLOR_CODE_FG,
            InlineStyle::Link => COLOR_LINK,
        };
        let mut w = TextWidget::new(run.text).color(color);
        if let Some(s) = font_size {
            w = w.font_size(s);
        }
        tree.add_child(row, w);
    }
}
