//! Main screen (Sidebar + Editor) — UI-only reproduction of `App.tsx`'s
//! `MainScreen` plus `Sidebar/Sidebar.tsx` and `Editor/Editor.tsx`.
//!
//! **Slice 1: the Sidebar shell.** Panel chrome, header action buttons, the
//! New Note button, the search box, a dummy note list.
//!
//! **Slice 2: the Editor pane** (`editor_pane`). The title section (bold title
//! input + pin/export/trash buttons + saved status + tag chips), the markdown
//! toolbar (format buttons split by vertical rules + a preview toggle pushed
//! right), the multi-line body, and the right-aligned status bar.
//!
//! Gaps surfaced while writing both slices are logged in
//! `docs/knot-ui-repro-gaps.md`:
//!   - **G3**  Input padding/height are fixed — the body wants `16px 24px`
//!     content padding and the title a tall `text-2xl` line; neither is met.
//!   - **G4**  asymmetric padding (`px-6 py-4`, `px-6 py-2`, `px-2 py-0.5`) —
//!     only uniform `Container::padding(px)` exists, so these are approximated.
//!   - **G6**  `Button` has no padding/height/disabled — `w-full py-2` is met
//!     only by column align-stretch; the `py-2` height is not controllable.
//!   - **G10** per-side border — `border-r`/`border-b`/`border-t` and the
//!     toolbar's vertical `w-px` rules are faked with 1px divider boxes
//!     (`Container::border` paints all four sides only).
//!   - **G11** only `*center` alignment — `justify-between` (header, toolbar)
//!     and `text-right` (status bar) are faked with `grow(1.0)` spacers.
//!   - **G12** `Input` has no font-weight — the `text-2xl font-bold` title
//!     can't be bold (`TextWidget` has `weight`, `Input` doesn't).
//!   - icons are approximate single glyphs (the real fix, a bundled icon font,
//!     is FW-12 and out of scope for this UI-only clone).
//!
//! The overlays — settings dropdown, error banner, context menu (slice 3) —
//! are still TODO.

use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::text::FontWeight;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, Input, ScrollView, TextWidget};

use crate::tokens;

/// A dummy note row: (title, relative date, tags, pinned).
type DummyNote = (&'static str, &'static str, &'static [&'static str], bool);

const NOTES: &[DummyNote] = &[
    (
        "Welcome to Knot",
        "2 hours ago",
        &["welcome", "guide"],
        true,
    ),
    ("Project ideas", "Yesterday", &["ideas"], false),
    ("Meeting notes — Q2 planning", "3 days ago", &[], false),
    ("Grocery list", "Apr 20, 2026", &[], false),
];
/// Which dummy note is shown as selected.
const SELECTED: usize = 0;

pub fn build(tree: &mut WidgetTree) {
    // MainScreen: `flex h-screen bg-gray-100 dark:bg-gray-900`.
    let root = tree.set_root(
        Container::row()
            .width_full()
            .height_full()
            .background(tokens::background()),
    );

    sidebar(tree, root);

    // `border-r border-gray-300 dark:border-gray-700` on the sidebar — faked
    // as a 1px divider column between sidebar and editor (gap G10).
    tree.add_child(
        root,
        Container::column()
            .width(1.0)
            .height_full()
            .background(border()),
    );

    editor_pane(tree, root);
}

// --- Sidebar (`w-64 bg-gray-200 dark:bg-gray-800 flex flex-col h-screen`) ---

fn sidebar(tree: &mut WidgetTree, parent: usize) {
    let side = tree.add_child(
        parent,
        Container::column()
            .width(256.0) // w-64
            .height_full()
            .background(panel()),
    );

    // --- Header (`p-4`, `mb-3` between the title row and New Note button) ---
    let header = tree.add_child(side, Container::column().padding(16.0).gap(12.0));

    // Title row: `flex items-center justify-between`. justify-between faked
    // with a grow spacer (gap G11).
    let title_row = tree.add_child(header, Container::row().align_center());
    tree.add_child(
        title_row,
        TextWidget::new("Knot")
            .font_size(18.0) // text-lg
            .weight(FontWeight::SEMIBOLD)
            .color(tokens::on_surface()),
    );
    tree.add_child(title_row, Container::row().grow(1.0)); // spacer → push group right

    // Action button group (`flex items-center gap-1`).
    let group = tree.add_child(title_row, Container::row().align_center().gap(4.0));
    for glyph in ["\u{2699}", "\u{22EE}", "\u{1F5D1}", "\u{1F512}"] {
        // ⚙ settings, ⋮ more, 🗑 trash, 🔒 lock
        icon_button(tree, group, glyph);
    }

    // New Note button (`w-full py-2 bg-blue-600 ... rounded-lg text-sm`).
    // w-full comes free from the column's stretch; py-2 height is not
    // controllable (gap G6).
    tree.add_child(
        header,
        Button::new("New Note")
            .radius(8.0)
            .background(tokens::primary())
            .hover_background(tokens::primary_hover())
            .text_color(Color::WHITE)
            .font_size(14.0)
            .on_click(|_ctx| { /* would create a note */ }),
    );

    // Header `border-b` (gap G10).
    divider(tree, side);

    // --- Search section (`p-3 border-b`) ---
    let search_sec = tree.add_child(side, Container::column().padding(12.0));
    search_bar(tree, search_sec);
    divider(tree, side);

    // --- Note list (`flex-1 overflow-y-auto`, inner `flex flex-col gap-1 p-2`) ---
    let list = tree.add_child(side, ScrollView::new().grow(1.0).padding(8.0).gap(4.0));
    for (i, note) in NOTES.iter().enumerate() {
        note_item(tree, list, note, i == SELECTED);
    }
}

/// Small square header button: `p-1.5 rounded`, transparent until hover.
/// `p-1.5` (6px) is uniform so it maps cleanly; the icon is a single glyph.
fn icon_button(tree: &mut WidgetTree, parent: usize, glyph: &str) {
    tree.add_child(
        parent,
        Button::new(glyph)
            .font_size(16.0)
            .radius(6.0)
            .background(Color::TRANSPARENT)
            .text_color(icon_fg())
            .hover_background(border())
            .on_click(|_ctx| {}),
    );
}

/// SearchBar: a bordered row (`px-3 py-2 border rounded-lg`) holding a
/// magnifier glyph + a borderless input.
///
/// This one element trips both G3 and G4 at once: `borderless()` drops the
/// frame but the Input keeps its hardcoded internal `padding(8)` +
/// `min_height(font+20)` (`input.rs:1469`), so a faithful "py-2 around a bare
/// input" double-pads vertically and the box balloons to ≈50px (React ≈36px).
/// And since `padding` is uniform, trimming the height (smaller pad) also
/// shrinks the magnifier's left inset — can't satisfy both. Compromise: pad 4
/// (≈42px), favoring height over inset.
fn search_bar(tree: &mut WidgetTree, parent: usize) {
    let bar = tree.add_child(
        parent,
        Container::row()
            .align_center()
            .gap(8.0)
            .padding(4.0) // see fn doc: G3 (Input min_height) + G4 (uniform pad)
            .radius(8.0)
            .background(tokens::pick(tokens::white(), tokens::gray_900()))
            .border(1.0, border()),
    );
    tree.add_child(
        bar,
        TextWidget::new("\u{1F50D}") // 🔍
            .font_size(14.0)
            .color(tokens::muted()),
    );
    tree.add_child(
        bar,
        Input::new()
            .borderless()
            .grow(1.0)
            .font_size(14.0)
            .background(Color::TRANSPARENT)
            .placeholder("Search notes..."),
    );
}

/// One note row: `w-full px-3 py-2 rounded-lg`, selected → solid bg, else
/// hover bg. `px-3 py-2` approximated by uniform padding (G4).
fn note_item(tree: &mut WidgetTree, parent: usize, note: &DummyNote, selected: bool) {
    let (title, date, tags, pinned) = *note;

    let mut row = Container::column().padding(8.0).radius(8.0).gap(2.0);
    if selected {
        row = row.background(tokens::pick(tokens::gray_300(), tokens::gray_700()));
    } else {
        row = row
            .hoverable()
            .hover_background(tokens::pick(tokens::gray_300(), tokens::gray_700()));
    }
    let item = tree.add_child(parent, row.on_press(|_pt, _ctx| { /* would select */ }));

    // Title line (`flex items-center gap-1`): optional pin glyph + title.
    let title_line = tree.add_child(item, Container::row().align_center().gap(4.0));
    if pinned {
        tree.add_child(
            title_line,
            TextWidget::new("\u{1F4CC}") // 📌
                .font_size(11.0)
                .color(tokens::muted()),
        );
    }
    tree.add_child(
        title_line,
        TextWidget::new(title)
            .font_size(14.0) // text-sm
            .weight(FontWeight::MEDIUM)
            .truncate(true)
            .color(tokens::on_surface()),
    );

    // Date (`text-xs text-gray-500`).
    tree.add_child(
        item,
        TextWidget::new(date).font_size(12.0).color(tokens::muted()),
    );

    // Tag chips (`flex gap-1 mt-1 flex-wrap`, each `rounded-full text-[10px]`).
    if !tags.is_empty() {
        let chips = tree.add_child(item, Container::row().gap(4.0).flex_wrap(true));
        for tag in tags {
            let chip = tree.add_child(
                chips,
                Container::row()
                    .padding(3.0) // ≈ px-1.5 py-0 (G4)
                    .radius(8.0) // rounded-full
                    .background(tokens::pick(tokens::gray_300(), tokens::gray_600())),
            );
            tree.add_child(
                chip,
                TextWidget::new(*tag).font_size(10.0).color(tokens::muted()),
            );
        }
    }
}

// --- Editor pane (`Editor/Editor.tsx`) ---------------------------------------
//
// `flex-1 min-w-0 bg-gray-50 dark:bg-gray-900 flex flex-col h-screen
// overflow-hidden`: a column that fills the remaining width, with a title
// section, a markdown toolbar, the body, and a status bar — each separated by
// a `border-b`/`border-t` (faked as 1px dividers, gap G10).

/// Dummy markdown shown in the editor body so the pane looks populated.
const BODY_TEXT: &str = "\
# Welcome to Knot

This is your first note. Knot keeps everything **encrypted** on your device — \
no plaintext ever touches disk.

- Markdown is supported
- [[Wikilinks]] connect your notes
- Drag in an image to embed it

Happy writing.";

fn editor_pane(tree: &mut WidgetTree, parent: usize) {
    let pane = tree.add_child(
        parent,
        Container::column()
            .grow(1.0)
            .height_full()
            .background(editor_bg()),
    );

    editor_title_section(tree, pane);
    divider(tree, pane); // title `border-b` (G10)
    editor_toolbar(tree, pane);
    divider(tree, pane); // toolbar `border-b` (G10)
    editor_body(tree, pane);
    divider(tree, pane); // status `border-t` (G10)
    editor_status_bar(tree, pane);
}

/// Title section (`border-b px-6 py-4`): the title input + action buttons, a
/// saved-status line, and the tag chips. `px-6 py-4` is asymmetric (G4); we use
/// uniform `padding(16)`.
fn editor_title_section(tree: &mut WidgetTree, parent: usize) {
    let sec = tree.add_child(parent, Container::column().padding(16.0).gap(8.0));

    // Title row (`flex items-center gap-3`).
    let row = tree.add_child(sec, Container::row().align_center().gap(12.0));
    // Title input: `flex-1 text-2xl font-bold bg-transparent border-none`. The
    // `font-bold` can't be expressed — Input has no weight builder (gap G12) —
    // and the fixed internal padding/height keep it from a true `text-2xl`
    // line (gap G3). Text color defaults to `on_surface` so it tracks the
    // theme toggle.
    tree.add_child(
        row,
        Input::new()
            .value(Signal::new("Welcome to Knot".to_string()))
            .borderless()
            .grow(1.0)
            .font_size(24.0) // text-2xl
            .background(Color::TRANSPARENT)
            .placeholder("Untitled note"),
    );
    // Pinned → blue (the selected note is pinned); export/trash → gray.
    flat_icon_button(tree, row, "\u{1F4CC}", 8.0, tokens::primary()); // 📌 pin
    flat_icon_button(tree, row, "\u{1F4E4}", 8.0, icon_fg()); // 📤 export
    flat_icon_button(tree, row, "\u{1F5D1}", 8.0, icon_fg()); // 🗑 delete

    // Saved status (`text-xs text-gray-500`). The 8px spacer aligns its text
    // with the title *input* above, whose fixed internal padding offsets the
    // text by 8px past the section padding (G3) — without it title and status
    // visibly stagger.
    let saved = tree.add_child(sec, Container::row().align_center());
    tree.add_child(saved, Container::row().width(8.0));
    tree.add_child(
        saved,
        TextWidget::new("Saved")
            .font_size(12.0)
            .color(tokens::muted()),
    );

    // Tag chips (`flex flex-wrap items-center gap-1.5`) + a borderless add box.
    // Leading 8px spacer matches the title-input inset (see "Saved" above).
    let tags = tree.add_child(
        sec,
        Container::row().align_center().gap(6.0).flex_wrap(true),
    );
    tree.add_child(tags, Container::row().width(8.0));
    for &tag in NOTES[SELECTED].2 {
        tag_chip(tree, tags, tag);
    }
    tree.add_child(
        tags,
        Input::new()
            .borderless()
            .grow(1.0)
            .font_size(12.0)
            .background(Color::TRANSPARENT)
            .placeholder("Add tags..."),
    );
}

/// A removable tag chip: `px-2 py-0.5 rounded-full bg-blue-100
/// dark:bg-blue-900/40 text-blue-700 dark:text-blue-300`, with a `×` button.
fn tag_chip(tree: &mut WidgetTree, parent: usize, tag: &str) {
    let chip = tree.add_child(
        parent,
        Container::row()
            .align_center()
            .gap(2.0)
            .padding(3.0) // ≈ px-2 py-0.5 (G4)
            .radius(10.0) // rounded-full
            .background(tokens::pick(
                tokens::blue_100(),
                // bg-blue-900/40 — solid blue-900 at 40% alpha.
                Color::from_rgba8(0x1e, 0x3a, 0x8a, 102),
            )),
    );
    let text = tokens::pick(tokens::blue_700(), tokens::blue_300());
    tree.add_child(
        chip,
        TextWidget::new(tag).font_size(12.0).color(text.clone()),
    );
    tree.add_child(
        chip,
        TextWidget::new("\u{00D7}").font_size(12.0).color(text),
    ); // ×
}

/// Markdown toolbar (`flex items-center gap-1 px-6 py-2 border-b`): format
/// buttons split into groups by vertical rules, then a `flex-1` spacer pushing
/// a preview toggle to the right (G11). `px-6 py-2` is asymmetric (G4).
fn editor_toolbar(tree: &mut WidgetTree, parent: usize) {
    let bar = tree.add_child(
        parent,
        Container::row().align_center().gap(4.0).padding(8.0),
    );
    tree.add_child(bar, Container::row().width(16.0)); // left inset → ≈px-6

    // Format glyphs are single-char stand-ins for the real SVG icons; their
    // differing advance widths make the buttons uneven (no icon font → FW-12;
    // and `Button` has no fixed-width builder → G6).
    for glyph in ["B", "I"] {
        flat_icon_button(tree, bar, glyph, 6.0, icon_fg());
    }
    toolbar_rule(tree, bar);
    for glyph in ["H", "\u{2022}", "1."] {
        // H heading, • bullet list, 1. numbered list
        flat_icon_button(tree, bar, glyph, 6.0, icon_fg());
    }
    toolbar_rule(tree, bar);
    for glyph in ["[[", "\u{1F517}", "\u{1F5BC}"] {
        // [[ wikilink, 🔗 external link, 🖼 image
        flat_icon_button(tree, bar, glyph, 6.0, icon_fg());
    }

    tree.add_child(bar, Container::row().grow(1.0)); // flex-1 → push toggle right
    flat_icon_button(tree, bar, "\u{1F441}", 6.0, icon_fg()); // 👁 preview toggle
    // Trailing 8px spacer so the preview button's right edge matches the title
    // row's delete button. The title section uses `padding(16)` (for `py-4`)
    // but the toolbar `padding(8)` (for `py-2`); uniform padding can't give
    // both the same horizontal inset (G4), so they'd otherwise stagger by 8px.
    tree.add_child(bar, Container::row().width(8.0));
}

/// Editor body (`flex-1 overflow-auto`, content `padding: 16px 24px`, 15px,
/// line-wrapped). A `grow` row holding a left-inset spacer + a borderless
/// multi-line input seeded with dummy markdown.
///
/// `grow(1.0)` (not `height_full`) is deliberate: `height_full` made the input
/// claim the *whole* pane height and paint over the toolbar/status dividers,
/// so `grow` (fill the remaining column space) is correct here. The fixed
/// internal padding still can't reach `16px 24px`, so a 16px spacer fakes the
/// `px-6` left inset (gaps G3 + G4).
fn editor_body(tree: &mut WidgetTree, parent: usize) {
    let row = tree.add_child(parent, Container::row().grow(1.0));
    tree.add_child(row, Container::row().width(16.0)); // left inset → text at ≈24px
    tree.add_child(
        row,
        Input::new()
            .value(Signal::new(BODY_TEXT.to_string()))
            .multiline()
            .borderless()
            .grow(1.0)
            .height_full()
            .font_size(15.0)
            .background(Color::TRANSPARENT)
            .placeholder("Start writing..."),
    );
}

/// Status bar (`border-t px-6 py-1 text-xs text-gray-400 text-right`). The
/// right alignment is faked with a leading `grow(1.0)` spacer (G11).
fn editor_status_bar(tree: &mut WidgetTree, parent: usize) {
    let row = tree.add_child(parent, Container::row().align_center().padding(4.0));
    tree.add_child(row, Container::row().grow(1.0)); // spacer → push text right
    tree.add_child(
        row,
        TextWidget::new("142 characters \u{00B7} 24 words")
            .font_size(12.0)
            .color(tokens::pick(tokens::gray_400(), tokens::gray_600())),
    );
}

/// Editor-pane background: `bg-gray-50 dark:bg-gray-900` (lighter than the
/// sidebar's `panel()` and distinct from the window `background()`).
fn editor_bg() -> shroud::reactive::Reactive<Color> {
    tokens::pick(tokens::gray_50(), tokens::gray_900())
}

/// A flat `p-2` icon button: transparent until hover, then `bg-gray-200
/// dark:bg-gray-800`. `radius` distinguishes `rounded-lg` (title row) from
/// `rounded` (toolbar); the glyph is a single-char stand-in (FW-12 territory).
fn flat_icon_button(
    tree: &mut WidgetTree,
    parent: usize,
    glyph: &str,
    radius: f32,
    text_color: shroud::reactive::Reactive<Color>,
) {
    tree.add_child(
        parent,
        Button::new(glyph)
            .font_size(16.0)
            .radius(radius)
            .background(Color::TRANSPARENT)
            .text_color(text_color)
            .hover_background(tokens::pick(tokens::gray_200(), tokens::gray_800()))
            .on_click(|_ctx| {}),
    );
}

/// A vertical `w-px h-5` rule separating toolbar groups (gap G10 — a 1px
/// sibling box, not a real per-side border).
fn toolbar_rule(tree: &mut WidgetTree, parent: usize) {
    tree.add_child(
        parent,
        Container::column()
            .width(1.0)
            .height(20.0)
            .background(border()),
    );
}

// --- shared chrome helpers ------------------------------------------------

/// Sidebar panel background: `bg-gray-200 dark:bg-gray-800`.
fn panel() -> shroud::reactive::Reactive<Color> {
    tokens::pick(tokens::gray_200(), tokens::gray_800())
}

/// Divider / border color: `border-gray-300 dark:border-gray-700`.
fn border() -> shroud::reactive::Reactive<Color> {
    tokens::pick(tokens::gray_300(), tokens::gray_700())
}

/// Icon-button foreground: `text-gray-600 dark:text-gray-400`. Distinct from
/// `tokens::muted()` (gray-500) — the muted shade reads too faint for glyphs.
fn icon_fg() -> shroud::reactive::Reactive<Color> {
    tokens::pick(tokens::gray_600(), tokens::gray_400())
}

/// A 1px horizontal rule standing in for a `border-b` (gap G10).
fn divider(tree: &mut WidgetTree, parent: usize) {
    tree.add_child(
        parent,
        Container::column()
            .width_full()
            .height(1.0)
            .background(border()),
    );
}
