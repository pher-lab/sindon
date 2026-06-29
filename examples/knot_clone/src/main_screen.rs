//! Main screen (Sidebar + Editor) — UI-only reproduction of `App.tsx`'s
//! `MainScreen` plus `Sidebar/Sidebar.tsx`.
//!
//! **Slice 1 (this file): the Sidebar shell + an editor placeholder.** It
//! reproduces the panel chrome, header action buttons, the New Note button,
//! the search box, and a dummy note list — enough to surface the layout gaps
//! the densest screen hits. Gaps found while writing it are logged in
//! `docs/knot-ui-repro-gaps.md`:
//!   - **G4**  asymmetric padding (`px-3 py-2`, `px-1.5 py-0`) — only uniform
//!     `Container::padding(px)` exists, so these are approximated.
//!   - **G6**  `Button` has no padding/height/disabled — `w-full py-2` is met
//!     only by column align-stretch; the `py-2` height is not controllable.
//!   - **G10** per-side border — `border-r`/`border-b` are faked with 1px
//!     divider boxes (`Container::border` paints all four sides only).
//!   - **G11** only `*center` alignment — `justify-between` is faked with a
//!     `grow(1.0)` spacer.
//!   - icons are approximate single glyphs (the real fix, a bundled icon font,
//!     is FW-12 and out of scope for this UI-only clone).
//!
//! Editor pane (slice 2) and the overlays — settings dropdown, error banner,
//! context menu (slice 3) — are still TODO.

use shroud::core::Color;
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

    editor_placeholder(tree, root);
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
            .text_color(tokens::muted())
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

// --- Editor pane placeholder (`flex-1` — real editor is slice 2) ---

fn editor_placeholder(tree: &mut WidgetTree, parent: usize) {
    let pane = tree.add_child(
        parent,
        Container::column()
            .grow(1.0)
            .height_full()
            .background(tokens::background())
            .center(),
    );
    tree.add_child(
        pane,
        TextWidget::new("Select a note to view")
            .font_size(16.0)
            .color(tokens::muted()),
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
