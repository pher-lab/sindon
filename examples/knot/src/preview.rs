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
//! GFM tables, task lists, and strikethrough (`~~text~~`) are rendered (see
//! [`options`]); strikethrough rides the framework's `TextSpan` decoration.
//! Standard `[text](url)` links are clickable and drawn underlined so they read
//! as links: an external web/mail target opens in the OS default handler (via
//! the `opener` crate), gated by a scheme allowlist so a note body can't launch
//! `file://` or some arbitrary-scheme handler. Other external targets (relative
//! paths, unknown schemes) are parsed and styled but inert.
//!
//! `[[Title]]` wikilinks navigate between notes. pulldown-cmark doesn't know
//! the syntax, so they're split out of plain text after parsing, rendered as
//! underlined accent links, and — when a [`WikiNav`] is supplied — clicking one
//! selects the note whose title matches and drops back into the editor,
//! mirroring a sidebar click. `[[Target|Alias]]` shows the alias while linking
//! the target. A wikilink to a title that doesn't exist renders normally but is
//! inert on click.
//!
//! Line breaks use "breaks" mode: a single newline renders as an actual line
//! break (not CommonMark's soft-break-as-space), since note writers expect
//! Enter to start a new line. A blank line is still a paragraph gap.
//!
//! Embedded images render from the encrypted attachment store: a
//! `![alt](knot-img:<id>)` reference is decrypted (via the preview's
//! [`WikiNav`] handle to live state) and drawn inline as its own block. An
//! image whose src is *not* a `knot-img:` reference — an http/file/data URL —
//! is **never fetched or read from disk**; it renders as an inert labeled
//! placeholder. That gate is the image analog of the link allowlist: a note
//! body must not be able to phone home (a tracking pixel) or pull an arbitrary
//! local file into view.
//!
//! Still out of scope: syntax highlighting. Strikethrough inside a `~~...~~`
//! span suppresses wikilink expansion (deleted text shouldn't sprout a live
//! link), so `~~[[x]]~~` stays literal.

use std::cell::RefCell;
use std::rc::Rc;

use std::sync::Arc;

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use shroud::core::Color;
use shroud::reactive::{Reactive, Signal};
use shroud::render::DecodedImage;
use shroud::text::TextSpan;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, Image, TextWidget};

use crate::settings;
use crate::state::{self, AppState, AttachmentId, Note, NoteId, Phase};

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

/// Internal target prefix marking a `[[wikilink]]`. The framework treats a
/// span's link target as an opaque string, so we tag wikilinks with a scheme
/// that `is_external_web_link` rejects (it can never reach the OS opener) and
/// strip it back off in `handle_link_click` to recover the note title.
const WIKI_SCHEME: &str = "knot-wiki:";

/// The preview's handle to live app state. It does double duty: it carries
/// what a clicked wikilink needs to switch the active note (shared state plus
/// the editor's bound signals), *and* it resolves `knot-img:<id>` references
/// against the encrypted attachment store (see [`WikiNav::resolve_image`]).
/// Cheap to clone (an `Rc` and three `Copy` signal handles), which matters
/// because every link/image block captures its own clone.
///
/// `None` is a valid "no live state" mode — the preview tests and any caller
/// that doesn't wire routing render wikilinks as inert accent text and embedded
/// images as "unavailable" placeholders.
#[derive(Clone)]
pub struct WikiNav {
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
}

impl WikiNav {
    pub fn new(
        state: Rc<RefCell<AppState>>,
        title_sig: Signal<String>,
        body_sig: Signal<String>,
        preview_sig: Signal<bool>,
    ) -> Self {
        Self {
            state,
            title_sig,
            body_sig,
            preview_sig,
        }
    }

    /// Select the note whose title matches `title` and return to the editor —
    /// the same end state as clicking that note in the sidebar. A title with
    /// no match is a no-op, so a dangling `[[wikilink]]` simply does nothing.
    fn navigate_to(&self, title: &str) {
        // Snapshot the match under a shared borrow, then re-borrow mutably to
        // flip the selection (RefCell panics on overlapping borrows).
        let snapshot = {
            let s = self.state.borrow();
            match &s.phase {
                Phase::Unlocked { notes, .. } => find_note_id_by_title(notes, title)
                    .and_then(|id| notes.iter().find(|n| n.id == id))
                    .map(|n| (n.id, n.title.clone(), n.body.clone())),
                _ => None,
            }
        };
        let Some((id, new_title, new_body)) = snapshot else {
            return;
        };
        {
            let mut s = self.state.borrow_mut();
            if let Phase::Unlocked { selected, .. } = &mut s.phase {
                *selected = Some(id);
            }
        }
        // Rebase the editor inputs and land on the editor (not a stale preview
        // of the note we just left), exactly as `sidebar::select_note` does.
        self.title_sig.set(new_title);
        self.body_sig.set(new_body);
        self.preview_sig.set(false);
    }

    /// Resolve an embedded-image attachment id to its decoded pixels via the
    /// live vault, decrypting (and caching) on first use. `None` when the id
    /// has no stored attachment, the bytes don't decode, or the app isn't
    /// unlocked — the caller then renders an "unavailable" placeholder.
    fn resolve_image(&self, id: AttachmentId) -> Option<Arc<DecodedImage>> {
        self.state.borrow_mut().resolve_attachment(id)
    }
}

/// First note whose title equals `target` (both trimmed, ASCII-case-
/// insensitive). ASCII folding lets English titles match regardless of case
/// while non-ASCII titles (e.g. Japanese) match exactly — the behavior a CJK
/// notes app wants, since there is no case to fold there.
fn find_note_id_by_title(notes: &[Note], target: &str) -> Option<NoteId> {
    let target = target.trim();
    notes
        .iter()
        .find(|n| n.title.trim().eq_ignore_ascii_case(target))
        .map(|n| n.id)
}

/// A run of plain text after `[[wikilink]]` spans have been split out.
enum WikiPiece {
    /// Literal text between (or around) wikilinks.
    Text(String),
    /// A `[[Target]]` / `[[Target|Display]]` link: `display` is shown, `target`
    /// is the note title to resolve.
    Link { display: String, target: String },
}

/// Split `text` on `[[Target]]` / `[[Target|Display]]` wikilink spans,
/// returning alternating plain and link pieces. An unterminated `[[`, an empty
/// target, or a candidate whose inner text contains a bracket or newline is
/// kept as literal text rather than guessed at.
fn split_wikilinks(text: &str) -> Vec<WikiPiece> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        let Some(close_rel) = after_open.find("]]") else {
            break; // no closer — the remainder is all literal text
        };
        let inner = &after_open[..close_rel];
        // A real wikilink can't be empty or carry stray brackets / newlines.
        if inner.is_empty() || inner.contains(['[', ']', '\n']) {
            // Keep the leading text and the literal "[[", then resume scanning
            // after the "[[" so the same opener is never re-tested (no spin).
            push_text(&mut pieces, &rest[..open + 2]);
            rest = after_open;
            continue;
        }
        if open > 0 {
            push_text(&mut pieces, &rest[..open]);
        }
        // `[[Target|Display]]` aliases the link text; bare `[[Target]]` shows
        // the target itself.
        let (target, display) = match inner.split_once('|') {
            Some((t, d)) => (t.trim(), d.trim()),
            None => (inner.trim(), inner.trim()),
        };
        if target.is_empty() {
            // e.g. `[[ |Display]]` — nothing to link to, keep it literal.
            push_text(&mut pieces, &rest[..open + 2 + close_rel + 2]);
        } else {
            let display = if display.is_empty() { target } else { display };
            pieces.push(WikiPiece::Link {
                display: display.to_string(),
                target: target.to_string(),
            });
        }
        rest = &after_open[close_rel + 2..];
    }
    push_text(&mut pieces, rest);
    pieces
}

/// Append `s` as text, merging into a trailing `Text` piece so consecutive
/// literal fragments don't fan out into many adjacent runs.
fn push_text(pieces: &mut Vec<WikiPiece>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(WikiPiece::Text(last)) = pieces.last_mut() {
        last.push_str(s);
    } else {
        pieces.push(WikiPiece::Text(s.to_string()));
    }
}

/// Replace each plain, not-already-linked run containing `[[` with its
/// wikilink-split pieces. Inline code, bold/italic, and text already inside a
/// markdown link are left untouched — so `[[x]]` inside `` `code` `` stays
/// literal and a wikilink can't nest inside another link.
fn expand_wikilinks(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        // Strikethrough runs are also spared: a wikilink inside `~~...~~` is
        // deleted text and shouldn't become a live navigation target.
        if run.style != InlineStyle::Plain
            || run.strikethrough
            || run.link.is_some()
            || !run.text.contains("[[")
        {
            out.push(run);
            continue;
        }
        for piece in split_wikilinks(&run.text) {
            match piece {
                WikiPiece::Text(text) => out.push(InlineRun {
                    text,
                    style: InlineStyle::Plain,
                    strikethrough: false,
                    link: None,
                }),
                WikiPiece::Link { display, target } => out.push(InlineRun {
                    text: display,
                    style: InlineStyle::Link,
                    strikethrough: false,
                    link: Some(format!("{WIKI_SCHEME}{target}")),
                }),
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
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
    /// Whether this run sits inside a `~~strikethrough~~` span. Orthogonal to
    /// `style` (which holds one of bold/italic/code/link) since strikethrough
    /// composes with any of them — `~~**bold**~~` is bold *and* struck.
    strikethrough: bool,
    /// Click target when this run sits inside a markdown link, else `None`.
    /// Carried separately from `style` so styled text inside a link (e.g.
    /// `[**bold**](url)`) stays clickable while keeping its own weight.
    link: Option<String>,
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
/// run's style and decoration, so a lone bold/italic/code/struck run would
/// render unstyled. Those go the rich path so the attribute survives.
fn is_fast_path(runs: &[InlineRun]) -> bool {
    runs.is_empty()
        || runs
            .iter()
            .all(|r| r.style == InlineStyle::Plain && !r.strikethrough)
}

/// GFM extensions the preview parser understands: tables, task lists, and
/// strikethrough (`~~text~~`), the last of which renders via the framework's
/// `TextSpan` decoration.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// Render `source` as markdown into `parent`. `parent` must be a column-flex
/// container; one block is appended per top-level markdown block. An empty /
/// whitespace-only source renders a single muted placeholder so the preview
/// pane never looks broken on a brand-new note.
pub fn render(tree: &mut WidgetTree, parent: usize, source: &str, nav: Option<&WikiNav>) {
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
        i = render_block(tree, parent, &events, i, nav);
    }
}

fn render_block(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    start: usize,
    nav: Option<&WikiNav>,
) -> usize {
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
            emit_inline_block(tree, parent, runs, Some(size), false, nav);
            end + 1
        }
        Event::Start(Tag::Paragraph) => {
            // Paragraphs can carry embedded images, which become their own
            // stacked blocks — so route through `render_paragraph`, which
            // splits the inline stream at image boundaries.
            let end = render_paragraph(tree, parent, events, start + 1, nav);
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
                j = render_block(tree, body, events, j, nav);
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
                        i = render_list_item(tree, item_body, events, body_start, nav);
                    }
                    Event::End(TagEnd::List(_)) => break,
                    _ => i += 1,
                }
            }
            i + 1
        }
        Event::Start(Tag::Table(aligns)) => render_table(tree, parent, events, start, aligns, nav),
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
fn render_list_item(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    start: usize,
    nav: Option<&WikiNav>,
) -> usize {
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(TagEnd::Item) => return i + 1,
            // pulldown-cmark wraps loose item text in a paragraph; tight lists
            // emit raw inline events directly inside Item.
            Event::Start(Tag::Paragraph) => {
                let end = render_paragraph(tree, parent, events, i + 1, nav);
                i = end + 1;
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Link { .. }) => {
                let (runs, end) = collect_inline(events, i, |t| matches!(t, TagEnd::Item));
                emit_inline_block(tree, parent, runs, None, false, nav);
                i = end;
            }
            Event::Start(Tag::List(_))
            | Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::BlockQuote)
            | Event::Start(Tag::Table(_)) => {
                i = render_block(tree, parent, events, i, nav);
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
    nav: Option<&WikiNav>,
) -> usize {
    let table = tree.add_child(parent, Container::column());
    let mut i = start + 1;
    let mut first_body_row = true;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::TableHead) => {
                i = render_table_row(tree, table, events, i + 1, aligns, true, nav);
                add_divider(tree, table);
            }
            Event::Start(Tag::TableRow) => {
                // Divider goes *between* body rows; the header already laid one
                // down, so the first body row skips its leading divider.
                if !first_body_row {
                    add_divider(tree, table);
                }
                first_body_row = false;
                i = render_table_row(tree, table, events, i + 1, aligns, false, nav);
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
    nav: Option<&WikiNav>,
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
                emit_inline_block(tree, cell_idx, runs, None, header, nav);
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

/// Append `text` to `runs`, merging into the trailing run when it shares the
/// same style *and* link. This matters for more than tidiness: pulldown-cmark
/// emits bracket-bearing plain text as one `Text` event per delimiter — e.g.
/// `[[Home]]` arrives as five events `"[" "[" "Home" "]" "]"`. Without
/// coalescing, each lands in its own run and `expand_wikilinks` never sees the
/// `[[…]]` pattern whole, so the wikilink is silently never detected.
fn push_run(
    runs: &mut Vec<InlineRun>,
    text: &str,
    style: InlineStyle,
    strikethrough: bool,
    link: Option<String>,
) {
    if let Some(last) = runs.last_mut() {
        if last.style == style && last.strikethrough == strikethrough && last.link == link {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(InlineRun {
        text: text.to_string(),
        style,
        strikethrough,
        link,
    });
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
    // Active link destinations, innermost last. Non-empty while inside one or
    // more `[..](dest)` spans; every text/code run emitted then carries the
    // current dest so it becomes a clickable region.
    let mut link_stack: Vec<String> = Vec::new();
    // Strikethrough nesting depth. Orthogonal to `style_stack` because GFM
    // strikethrough composes with bold/italic/links rather than replacing them.
    let mut strike_depth = 0u32;
    // Image nesting depth. An image's alt text arrives as `Text` events
    // between `Start(Image)` and `End(Image)`; while inside one we drop that
    // text so the alt never leaks into the body. Paragraphs render images as
    // real blocks via `render_paragraph` (which splits before reaching here);
    // this guard covers the contexts that don't — headings, table cells, and
    // tight list items — where an image is simply suppressed rather than shown.
    let mut image_depth = 0u32;
    let mut i = start;
    while i < events.len() {
        let strike = strike_depth > 0;
        match &events[i] {
            Event::End(t) if is_end(t) => return (runs, i),
            Event::Text(s) if image_depth == 0 => {
                let style = *style_stack.last().unwrap();
                push_run(&mut runs, s, style, strike, link_stack.last().cloned());
            }
            Event::Code(s) if image_depth == 0 => {
                push_run(
                    &mut runs,
                    s,
                    InlineStyle::Code,
                    strike,
                    link_stack.last().cloned(),
                );
            }
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            // Render every line break as an actual line break ("breaks" mode),
            // not CommonMark's soft-break-as-space. A note app's users expect
            // Enter to start a new line, so `文字\n文字` should break rather than
            // join with a space; a blank line is still a paragraph gap. The
            // emitted "\n" is honored by cosmic-text in both the plain and rich
            // shaping paths. (A HardBreak — `  \n` / `\` — must break too, so
            // both arms map the same way.)
            Event::SoftBreak | Event::HardBreak => {
                push_run(&mut runs, "\n", InlineStyle::Plain, strike, None);
            }
            Event::Start(Tag::Strong) => style_stack.push(InlineStyle::Bold),
            Event::Start(Tag::Emphasis) => style_stack.push(InlineStyle::Italic),
            Event::Start(Tag::Strikethrough) => strike_depth += 1,
            Event::Start(Tag::Link { dest_url, .. }) => {
                style_stack.push(InlineStyle::Link);
                link_stack.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                style_stack.pop();
                link_stack.pop();
            }
            Event::End(TagEnd::Strong | TagEnd::Emphasis) => {
                style_stack.pop();
            }
            Event::End(TagEnd::Strikethrough) => strike_depth = strike_depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    (runs, i)
}

/// Upper cap on an embedded image's rendered width (px), passed to
/// [`Image::max_width`]. The image fills the preview column up to this cap,
/// scaling *down* to the column when it is narrower (so a wide image in a
/// narrow window shrinks to fit instead of overflowing and inflating the
/// column's height), and never upscaling a smaller image past its natural
/// size. Keeps long-form images readable without letting them dominate a wide
/// window.
const MAX_PREVIEW_IMAGE_WIDTH: f32 = 480.0;

/// Render a paragraph's inline content, breaking it into vertically stacked
/// blocks at embedded-image boundaries. Text segments go through the usual
/// inline path (`collect_inline` + `emit_inline_block`); each image renders as
/// its own block via `emit_image_block`. Returns the index of the paragraph's
/// closing `TagEnd::Paragraph` (the caller adds the `+ 1`).
///
/// Most images sit alone in a paragraph (`![alt](url)` on its own line), so the
/// common result is a single image block. A mixed `text ![img] text` paragraph
/// degrades to three stacked blocks rather than a true inline image — there is
/// no inline-image-in-a-text-run primitive, and note writers rarely interleave.
fn render_paragraph(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    start: usize,
    nav: Option<&WikiNav>,
) -> usize {
    let mut seg_start = start;
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(TagEnd::Paragraph) => {
                flush_text_segment(tree, parent, events, seg_start, i, nav);
                return i;
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                flush_text_segment(tree, parent, events, seg_start, i, nav);
                let dest = dest_url.to_string();
                let (alt, img_end) = collect_image_alt(events, i + 1);
                emit_image_block(tree, parent, &dest, &alt, nav);
                i = img_end + 1;
                seg_start = i;
            }
            _ => i += 1,
        }
    }
    // Unterminated paragraph (not expected from a well-formed parse): flush
    // the remainder and report the end of the stream.
    flush_text_segment(tree, parent, events, seg_start, events.len(), nav);
    events.len()
}

/// Emit the text segment `[from, to)` of a paragraph as one inline block, or
/// nothing when it is empty or only whitespace / line breaks — so the gaps
/// around a standalone image don't fan out into empty blocks.
fn flush_text_segment(
    tree: &mut WidgetTree,
    parent: usize,
    events: &[Event],
    from: usize,
    to: usize,
    nav: Option<&WikiNav>,
) {
    if to <= from {
        return;
    }
    // `collect_inline` over the sub-slice ending at `to`, with an end predicate
    // that never fires, walks exactly `[from, to)`.
    let (runs, _end) = collect_inline(&events[..to], from, |_| false);
    if runs
        .iter()
        .all(|r| r.text.trim().is_empty() && r.link.is_none())
    {
        return;
    }
    emit_inline_block(tree, parent, runs, None, false, nav);
}

/// Collect an image's alt text — the `Text` / `Code` events between
/// `Start(Image)` and its `End(Image)` — and return it with the index of the
/// closing `TagEnd::Image`.
fn collect_image_alt(events: &[Event], start: usize) -> (String, usize) {
    let mut alt = String::new();
    let mut i = start;
    while i < events.len() {
        match &events[i] {
            Event::End(TagEnd::Image) => return (alt, i),
            Event::Text(t) | Event::Code(t) => alt.push_str(t),
            _ => {}
        }
        i += 1;
    }
    (alt, i)
}

/// Emit one embedded-image block. A `knot-img:<id>` reference is decrypted via
/// `nav` and drawn; a missing attachment (or no live `nav`, e.g. in tests)
/// shows an "unavailable" placeholder. Any other src is treated as an
/// *external* image — never fetched or read from disk — and shows an inert
/// labeled placeholder, the image analog of the link allowlist.
fn emit_image_block(
    tree: &mut WidgetTree,
    parent: usize,
    dest: &str,
    alt: &str,
    nav: Option<&WikiNav>,
) {
    if let Some(id) = state::parse_attachment_ref(dest) {
        if let Some(img) = nav.and_then(|n| n.resolve_image(id)) {
            // Responsive: fill the preview column up to the width cap, scaling
            // the image (and its height, aspect preserved) down to a narrower
            // column rather than overflowing it. Never upscales a small image.
            tree.add_child(
                parent,
                Image::from_decoded(img).max_width(MAX_PREVIEW_IMAGE_WIDTH),
            );
        } else {
            tree.add_child(
                parent,
                TextWidget::new("[image unavailable]").color(muted_color()),
            );
        }
        return;
    }
    // External / unknown src: surface the alt (or raw src) so the reader knows
    // an image was intended, but never touch the network or disk.
    let label = if alt.trim().is_empty() { dest } else { alt };
    tree.add_child(
        parent,
        TextWidget::new(format!("[external image: {label}]")).color(muted_color()),
    );
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
    nav: Option<&WikiNav>,
) {
    // Pull `[[wikilinks]]` out of plain text into their own Link runs before
    // anything else, so the fast-path check below sees them as styled runs and
    // routes the block through the rich (clickable) path.
    let runs = expand_wikilinks(runs);
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
    let has_link = runs.iter().any(|r| r.link.is_some());
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
            if run.strikethrough {
                span = span.strikethrough();
            }
            if let Some(dest) = run.link {
                // Make this span clickable and underline it so it reads as a
                // link; `handle_link_click` decides what (if anything) the
                // target does when the framework reports the click back to us.
                span = span.link(dest).underline();
            }
            span
        })
        .collect();

    let mut w = TextWidget::rich(spans).color(body_color());
    if let Some(s) = font_size {
        w = w.font_size(s).line_height(heading_line_height(s));
    }
    if has_link {
        // Clone the nav (if any) into the click closure: it must be `'static`
        // since the widget outlives this call, and each link block keeps its
        // own copy.
        let nav = nav.cloned();
        w = w.on_link_click(move |target, _ctx| handle_link_click(target, nav.as_ref()));
    }
    tree.add_child(parent, w);
}

/// Act on a click of a preview link.
///
/// A `[[wikilink]]` (carried as `WIKI_SCHEME` + title) navigates to the
/// matching note when a [`WikiNav`] is wired. External web/mail targets open in
/// the OS default handler. Everything else is ignored: relative paths, fragment
/// links, and unknown schemes have no meaning here, and refusing them keeps a
/// note body from launching `file://` or some arbitrary-scheme handler.
fn handle_link_click(target: &str, nav: Option<&WikiNav>) {
    if let Some(title) = target.strip_prefix(WIKI_SCHEME) {
        if let Some(nav) = nav {
            nav.navigate_to(title);
        }
        return;
    }
    if is_external_web_link(target) {
        // Best-effort: a failed launch (no handler / sandbox) is swallowed —
        // there is no UI surface to report it and a dead link must never panic
        // the editor.
        let _ = opener::open(target);
    }
}

/// Whether `target` is a web/mail URL we're willing to hand to the OS opener.
/// Deliberately a small allowlist (http/https/mailto) rather than "anything
/// with a scheme" — a privacy-first notes app should not let body text trigger
/// arbitrary protocol handlers.
fn is_external_web_link(target: &str) -> bool {
    let t = target.trim().to_ascii_lowercase();
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("mailto:")
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
        render(&mut tree, col, source, None);

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
    fn link_run_captures_its_destination() {
        // A `[text](url)` link must surface as a Link-styled run carrying its
        // destination, so the emitted span becomes clickable.
        let evs: Vec<Event> =
            Parser::new_ext("[click me](https://example.com)", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert!(
            runs.iter().any(|r| r.style == InlineStyle::Link
                && r.link.as_deref() == Some("https://example.com")),
            "link text run should carry its dest_url, got {:?}",
            runs.iter()
                .map(|r| (&r.text, r.style, &r.link))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_link_text_has_no_destination() {
        // Plain text outside any link must not pick up a stray dest.
        let evs: Vec<Event> = Parser::new_ext("just plain words", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert!(runs.iter().all(|r| r.link.is_none()));
    }

    #[test]
    fn only_web_and_mail_schemes_are_openable() {
        // Security gate: a note body must only be able to launch http/https/
        // mailto, never file://, javascript:, custom schemes, or relatives.
        assert!(is_external_web_link("https://example.com"));
        assert!(is_external_web_link("http://example.com/path?q=1"));
        assert!(is_external_web_link("HTTPS://EXAMPLE.COM")); // case-insensitive
        assert!(is_external_web_link("mailto:a@b.com"));
        assert!(is_external_web_link("  https://leading-space.example  "));

        assert!(!is_external_web_link("file:///etc/passwd"));
        assert!(!is_external_web_link("javascript:alert(1)"));
        assert!(!is_external_web_link("ftp://example.com"));
        assert!(!is_external_web_link("knot://note/123"));
        assert!(!is_external_web_link("../relative/path"));
        assert!(!is_external_web_link("#fragment"));
        assert!(!is_external_web_link(""));
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
            strikethrough: false,
            link: None,
        }];
        assert!(
            !is_fast_path(&bold_only),
            "a lone bold run must render rich, not as plain text"
        );

        let plain_only = vec![InlineRun {
            text: "hello".into(),
            style: InlineStyle::Plain,
            strikethrough: false,
            link: None,
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
                strikethrough: false,
                link: None,
            },
            InlineRun {
                text: "b".into(),
                style: InlineStyle::Code,
                strikethrough: false,
                link: None,
            },
        ];
        assert!(!is_fast_path(&mixed), "a styled run anywhere forces rich");

        // A plain run that's struck through must also leave the fast path, or
        // the strike-through (a rich-only decoration) is silently dropped.
        let struck = vec![InlineRun {
            text: "gone".into(),
            style: InlineStyle::Plain,
            strikethrough: true,
            link: None,
        }];
        assert!(
            !is_fast_path(&struck),
            "a struck plain run must render rich so the decoration survives"
        );
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

    // ── Wikilink parsing ────────────────────────────────────────────────────

    /// Collapse pieces to a debuggable shape: `("text", None)` for literals and
    /// `("display", Some("target"))` for links.
    fn pieces(text: &str) -> Vec<(String, Option<String>)> {
        split_wikilinks(text)
            .into_iter()
            .map(|p| match p {
                WikiPiece::Text(t) => (t, None),
                WikiPiece::Link { display, target } => (display, Some(target)),
            })
            .collect()
    }

    #[test]
    fn wikilink_bare_target_links_to_itself() {
        assert_eq!(
            pieces("[[My Note]]"),
            vec![("My Note".to_string(), Some("My Note".to_string()))]
        );
    }

    #[test]
    fn wikilink_alias_shows_display_links_target() {
        // `[[Target|Display]]` renders Display but resolves Target.
        assert_eq!(
            pieces("[[real-title|click here]]"),
            vec![("click here".to_string(), Some("real-title".to_string()))]
        );
    }

    #[test]
    fn wikilink_keeps_surrounding_text_as_literals() {
        assert_eq!(
            pieces("see [[A]] and [[B]] done"),
            vec![
                ("see ".to_string(), None),
                ("A".to_string(), Some("A".to_string())),
                (" and ".to_string(), None),
                ("B".to_string(), Some("B".to_string())),
                (" done".to_string(), None),
            ]
        );
    }

    #[test]
    fn malformed_wikilinks_stay_literal() {
        // Unterminated, empty, bracket-laden, and target-less candidates are
        // never guessed at — they round-trip as plain text.
        for raw in [
            "a [[b without close",
            "[[]]",
            "[[ |only-alias]]",
            "[[has[bracket]]",
            "plain text, no links",
        ] {
            let collapsed: String = split_wikilinks(raw)
                .into_iter()
                .map(|p| match p {
                    WikiPiece::Text(t) => t,
                    WikiPiece::Link { .. } => panic!("`{raw}` should not parse as a wikilink"),
                })
                .collect();
            assert_eq!(collapsed, raw, "literal text must round-trip unchanged");
        }
    }

    #[test]
    fn expand_wikilinks_splits_plain_but_spares_code_and_links() {
        let runs = vec![
            InlineRun {
                text: "go to [[Home]]".into(),
                style: InlineStyle::Plain,
                strikethrough: false,
                link: None,
            },
            // Inline code carrying `[[x]]` must stay literal, not linkify.
            InlineRun {
                text: "[[notalink]]".into(),
                style: InlineStyle::Code,
                strikethrough: false,
                link: None,
            },
        ];
        let out = expand_wikilinks(runs);
        // "go to " (plain) + "Home" (link) + the untouched code run.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].style, InlineStyle::Plain);
        assert_eq!(out[0].link, None);
        assert_eq!(out[1].style, InlineStyle::Link);
        assert_eq!(
            out[1].link.as_deref(),
            Some("knot-wiki:Home"),
            "a wikilink run carries the internal scheme + bare title"
        );
        assert_eq!(out[2].style, InlineStyle::Code);
        assert_eq!(out[2].text, "[[notalink]]");
    }

    #[test]
    fn wikilink_target_is_inert_to_the_external_opener() {
        // The internal scheme must never satisfy the OS-opener allowlist, so a
        // note body can't smuggle a launch through a wikilink.
        assert!(!is_external_web_link(&format!("{WIKI_SCHEME}Home")));
    }

    #[test]
    fn fragmented_brackets_coalesce_so_real_wikilinks_are_detected() {
        // Regression for the live "wikilink doesn't become a link" bug: parsed
        // through the *real* pulldown-cmark, `[[Home]]` arrives as five separate
        // Text events ("[", "[", "Home", "]", "]"). collect_inline must coalesce
        // them into one plain run, or expand_wikilinks can never see the
        // `[[...]]` pattern whole. (The earlier wikilink tests fed hand-built
        // strings straight to split_wikilinks and so missed this entirely —
        // exercise the parser, not just the splitter.)
        let evs: Vec<Event> = Parser::new_ext("see [[Home]] here", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert_eq!(
            runs.len(),
            1,
            "the five bracket fragments must merge into one plain run, got {:?}",
            runs.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
        assert_eq!(runs[0].text, "see [[Home]] here");

        let out = expand_wikilinks(runs);
        assert!(
            out.iter().any(
                |r| r.style == InlineStyle::Link && r.link.as_deref() == Some("knot-wiki:Home")
            ),
            "the coalesced run must expand into a Home wikilink, got {:?}",
            out.iter()
                .map(|r| (&r.text, r.style, &r.link))
                .collect::<Vec<_>>()
        );
    }

    // ── Strikethrough ───────────────────────────────────────────────────────

    #[test]
    fn strikethrough_text_is_detected_through_the_real_parser() {
        // `~~gone~~` must surface as a single struck run carrying the inner
        // text — exercised through the actual parser, since strikethrough only
        // emits its events when ENABLE_STRIKETHROUGH is on in `options()`.
        let evs: Vec<Event> = Parser::new_ext("~~gone~~", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert_eq!(runs.len(), 1, "one struck run, got {:?}", runs_dump(&runs));
        assert!(runs[0].strikethrough, "the run must be marked struck");
        assert_eq!(runs[0].text, "gone");
    }

    #[test]
    fn strikethrough_composes_with_bold() {
        // `~~**b**~~` is both struck *and* bold — strikethrough is orthogonal
        // to the run's style, not a replacement for it.
        let evs: Vec<Event> = Parser::new_ext("~~**b**~~", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert!(
            runs.iter()
                .any(|r| r.strikethrough && r.style == InlineStyle::Bold),
            "expected a struck bold run, got {:?}",
            runs_dump(&runs)
        );
    }

    #[test]
    fn strikethrough_forces_the_rich_path() {
        // A lone struck run can't take the plain fast path, or the decoration
        // is dropped (paralleling the lone-bold-run case).
        let evs: Vec<Event> = Parser::new_ext("~~deleted~~", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert!(
            !is_fast_path(&runs),
            "a struck run must route through the rich path, got {:?}",
            runs_dump(&runs)
        );
    }

    #[test]
    fn strikethrough_suppresses_wikilink_expansion() {
        // `~~[[Home]]~~` is deleted text: the wikilink inside must stay literal
        // rather than expand into a live navigation target.
        let evs: Vec<Event> = Parser::new_ext("~~[[Home]]~~", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        let out = expand_wikilinks(runs);
        assert!(
            out.iter().all(|r| r.style != InlineStyle::Link),
            "a wikilink inside strikethrough must not become a link, got {:?}",
            runs_dump(&out)
        );
    }

    #[test]
    fn strikethrough_block_renders_without_panicking() {
        // End-to-end through the renderer: a struck paragraph still produces a
        // real, positive-height block.
        assert!(rendered_height("This is ~~deleted~~ text.").size_is_positive());
    }

    /// Compact `(text, style, struck, link)` dump for assertion messages.
    fn runs_dump(runs: &[InlineRun]) -> Vec<(&str, InlineStyle, bool, Option<&str>)> {
        runs.iter()
            .map(|r| (r.text.as_str(), r.style, r.strikethrough, r.link.as_deref()))
            .collect()
    }

    // ── Line breaks ─────────────────────────────────────────────────────────

    #[test]
    fn single_newline_renders_as_a_line_break_not_a_space() {
        // "breaks" mode: a lone `\n` inside a paragraph must survive as a real
        // newline in the shaped run, not collapse to a space (the CommonMark
        // default that surprised note writers).
        let evs: Vec<Event> = Parser::new_ext("line one\nline two", options()).collect();
        let (runs, _end) = collect_inline(&evs, 1, |t| matches!(t, TagEnd::Paragraph));
        assert_eq!(runs.len(), 1, "got {:?}", runs_dump(&runs));
        assert_eq!(
            runs[0].text, "line one\nline two",
            "the soft break must stay a newline, not become a space"
        );
    }

    #[test]
    fn single_newline_paragraph_is_taller_than_one_line() {
        // End-to-end proof the break actually wraps to a second visual line:
        // `a\nb` must out-measure `a b`, which fits on one line.
        let two_lines = rendered_height("a\nb");
        let one_line = rendered_height("a b");
        assert!(
            two_lines > one_line,
            "a single-newline paragraph should be taller ({two_lines}) than a one-liner ({one_line})"
        );
    }

    // ── Title lookup ────────────────────────────────────────────────────────

    fn note(id: NoteId, title: &str) -> Note {
        Note {
            id,
            title: title.to_string(),
            body: String::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn title_lookup_matches_trimmed_and_ascii_case_insensitive() {
        let notes = vec![note(1, "Inbox"), note(2, "Project Plan"), note(3, "日記")];
        assert_eq!(find_note_id_by_title(&notes, "Inbox"), Some(1));
        assert_eq!(find_note_id_by_title(&notes, "  project plan  "), Some(2));
        // Non-ASCII (Japanese) matches exactly — there's no case to fold.
        assert_eq!(find_note_id_by_title(&notes, "日記"), Some(3));
        assert_eq!(find_note_id_by_title(&notes, "missing"), None);
    }

    #[test]
    fn title_lookup_returns_the_first_match_on_duplicates() {
        // Duplicate titles are legal in Knot; a wikilink resolves to the first.
        let notes = vec![note(7, "Notes"), note(9, "Notes")];
        assert_eq!(find_note_id_by_title(&notes, "Notes"), Some(7));
    }

    #[test]
    fn wikilink_block_renders_without_panicking() {
        // End-to-end through the renderer (nav: None — inert click): a body that
        // is just a wikilink must still produce a real, positive-height block.
        assert!(rendered_height("Jump to [[Some Note]] now.").size_is_positive());
    }

    // ── Embedded images ─────────────────────────────────────────────────────

    #[test]
    fn standalone_attachment_image_is_one_block() {
        // `![alt](knot-img:1)` alone in a paragraph renders exactly one block.
        // With no live nav the attachment can't resolve, so it's the
        // "unavailable" placeholder — and crucially NOT the alt text leaked as
        // a stray paragraph (alt is consumed by `collect_image_alt`).
        let (tree, col) = build("![my picture](knot-img:1)");
        let blocks = tree.children(col);
        assert_eq!(blocks.len(), 1, "a standalone image is one block");
    }

    #[test]
    fn mixed_text_and_image_paragraph_splits_into_stacked_blocks() {
        // "before ![x](knot-img:1) after" degrades to three stacked blocks:
        // text, image, text (no inline-image-in-a-run primitive).
        let (tree, col) = build("before ![x](knot-img:1) after");
        let blocks = tree.children(col);
        assert_eq!(blocks.len(), 3, "text / image / text");
    }

    #[test]
    fn images_lay_out_and_external_sources_stay_inert() {
        // An internal ref (unresolvable here), an http URL, and a file path all
        // produce a real block without panicking — and the external ones go the
        // inert-placeholder path: the renderer never fetches or reads them.
        assert!(rendered_height("![a](knot-img:1)").size_is_positive());
        assert!(rendered_height("![a](https://example.com/a.png)").size_is_positive());
        assert!(rendered_height("![a](file:///etc/passwd)").size_is_positive());
        assert!(rendered_height("![a](data:image/png;base64,AAAA)").size_is_positive());
    }

    #[test]
    fn external_image_routes_away_from_the_attachment_path() {
        // The gate is in `parse_attachment_ref`: only `knot-img:` is treated as
        // an attachment; every external scheme returns None and so renders as an
        // inert placeholder rather than reaching `resolve_image`.
        assert_eq!(crate::state::parse_attachment_ref("knot-img:5"), Some(5));
        assert!(crate::state::parse_attachment_ref("https://example.com/a.png").is_none());
        assert!(crate::state::parse_attachment_ref("file:///etc/passwd").is_none());
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
