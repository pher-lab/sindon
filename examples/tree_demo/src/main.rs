//! tree_demo — Tier 2 collapsible tree view.
//!
//! A file-tree-shaped `TreeView` in a scrolling sidebar panel. Click a chevron
//! to expand/collapse a folder; click a row to select it (the selection label
//! updates from the `on_select` callback). The tree is deliberately taller than
//! its panel so the keyboard's scroll-into-view is visible.
//!
//! Visual check (mouse):
//! - parent rows show a chevron (▸ closed, ▾ open); leaves have none and align
//!   with their siblings' labels;
//! - clicking a chevron toggles that node without changing the selection;
//! - clicking a row highlights it and updates the "Selected:" line;
//! - nested folders indent one step per level, and deep rows clip at the panel
//!   edge rather than spilling.
//!
//! Visual check (keyboard):
//! - Tab reaches the tree once — not once per row — and a focus ring appears on
//!   one row: the cursor;
//! - ↑/↓ move that ring without changing the "Selected:" line, and pull the
//!   panel along when the cursor walks past the bottom edge;
//! - → opens a closed folder, then steps into it; ← closes an open one, then
//!   climbs back out to the parent;
//! - Enter or Space commits the cursor row, updating "Selected:";
//! - typing `r` jumps to the next row starting with r (again to cycle), and
//!   `re` refines to `render`/`README.md`;
//! - expanding with → keeps the keyboard: the arrows still work straight after.
//!
//! Screen-reader check (Windows Narrator, Ctrl+Win+Enter — a11y is judged by
//! listening, never by a screenshot):
//! - Tab into the tree: it announces the tree by name ("Project files") and then
//!   the cursor row — not just the container, and then silence;
//! - ↑/↓: each row is announced as you arrive, with its level ("level 2");
//! - →/←: opening and closing are announced as expanded / collapsed, and a leaf
//!   offers neither;
//! - Enter: the row is announced as selected;
//! - Narrator's own item commands (scan mode, or "expand"/"collapse") operate the
//!   tree without the keyboard cursor and selection drifting apart.

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, ScrollView, TextWidget, TreeItem, TreeView};

const BG: Color = Color::rgb(0.10, 0.11, 0.15);
const PANEL: Color = Color::rgb(0.13, 0.14, 0.19);
const HEADING: Color = Color::rgb(0.92, 0.94, 1.0);
const MUTED: Color = Color::rgb(0.7, 0.72, 0.8);

fn model() -> Vec<TreeItem> {
    vec![
        TreeItem::with_children(
            1,
            "src",
            vec![
                TreeItem::new(2, "main.rs"),
                TreeItem::with_children(
                    3,
                    "widgets",
                    vec![
                        TreeItem::new(4, "button.rs"),
                        TreeItem::new(5, "tree_view.rs"),
                        TreeItem::new(6, "split.rs"),
                    ],
                ),
                TreeItem::with_children(
                    7,
                    "render",
                    vec![TreeItem::new(8, "atlas.rs"), TreeItem::new(9, "glyph.rs")],
                ),
            ],
        ),
        TreeItem::with_children(
            10,
            "docs",
            vec![
                TreeItem::new(11, "roadmap.md"),
                TreeItem::new(12, "dogfood-log.md"),
            ],
        ),
        TreeItem::with_children(
            15,
            "tests",
            vec![
                TreeItem::new(16, "focus_reveal_tests.rs"),
                TreeItem::new(17, "layer_tests.rs"),
                TreeItem::new(18, "widget_tests.rs"),
            ],
        ),
        TreeItem::new(13, "Cargo.toml"),
        TreeItem::new(14, "README.md"),
    ]
}

fn label_for(id: u64) -> &'static str {
    match id {
        1 => "src",
        2 => "main.rs",
        3 => "widgets",
        4 => "button.rs",
        5 => "tree_view.rs",
        6 => "split.rs",
        7 => "render",
        8 => "atlas.rs",
        9 => "glyph.rs",
        10 => "docs",
        11 => "roadmap.md",
        12 => "dogfood-log.md",
        13 => "Cargo.toml",
        14 => "README.md",
        15 => "tests",
        16 => "focus_reveal_tests.rs",
        17 => "layer_tests.rs",
        18 => "widget_tests.rs",
        _ => "?",
    }
}

fn main() {
    let selected = Signal::new(None::<u64>);

    App::new()
        .title("shroud — tree view (Tier 2)")
        .size(420, 560)
        .run(move |_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(20.0)
                    .gap(14.0)
                    .background(BG),
            );

            tree.add_child(
                root,
                TextWidget::new("Project").font_size(22.0).color(HEADING),
            );

            let panel = tree.add_child(
                root,
                Container::column()
                    .width_full()
                    .grow(1.0)
                    .padding(8.0)
                    .radius(10.0)
                    .overflow_hidden()
                    .background(PANEL),
            );

            // The rows live in a scroll viewport: expanded, the tree is taller
            // than the panel, so arrowing off the bottom edge has to bring the
            // cursor row back into view.
            let viewport = tree.add_child(panel, ScrollView::new().width_full().grow(1.0));

            let sel = selected;
            TreeView::new(model())
                // Names the tree for a screen reader; nothing is painted, so
                // this echoes the visible "Project" heading above the panel.
                .label("Project files")
                .expanded([1, 3, 7, 15])
                .selected(5)
                .on_select(move |id, _ctx| sel.set(Some(id)))
                .build(&mut tree, viewport);

            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    let name = selected.get().map(label_for).unwrap_or("(none)");
                    format!("Selected: {name}")
                })
                .font_size(14.0)
                .color(MUTED),
            );

            tree
        });
}
