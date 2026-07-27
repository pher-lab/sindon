//! split_demo — Tier 2 resizable split panes.
//!
//! A horizontal split (sidebar | main) whose right pane is itself a vertical
//! split (editor over preview). Drag either divider to reapportion; the panes
//! reflow live and clip their own content.
//!
//! Visual check:
//! - dragging the vertical divider between the sidebar and the main area
//!   resizes both, and the divider thickens / recolors while hovered or dragged;
//! - dragging the horizontal divider in the main area resizes editor vs preview;
//! - each region clips its content to its box (nothing bleeds across a divider);
//! - releasing the mouse off the thin handle still ends the drag cleanly.

use sindon::app::App;
use sindon::core::Color;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, SplitPane, TextWidget};

const BG: Color = Color::rgb(0.10, 0.11, 0.15);
const SIDEBAR: Color = Color::rgb(0.13, 0.14, 0.19);
const EDITOR: Color = Color::rgb(0.16, 0.17, 0.22);
const PREVIEW: Color = Color::rgb(0.12, 0.16, 0.20);
const HEADING: Color = Color::rgb(0.92, 0.94, 1.0);
const BODY: Color = Color::rgb(0.72, 0.75, 0.82);

/// A labeled, padded fill for a pane, so its bounds and clipping are obvious.
fn panel(tree: &mut WidgetTree, pane: usize, bg: Color, title: &str, body: &str) {
    let col = tree.add_child(
        pane,
        Container::column()
            .width_full()
            .height_full()
            .padding(16.0)
            .gap(10.0)
            .background(bg),
    );
    tree.add_child(col, TextWidget::new(title).font_size(18.0).color(HEADING));
    tree.add_child(col, TextWidget::new(body).font_size(14.0).color(BODY));
}

fn main() {
    App::new()
        .title("sindon — split panes (Tier 2)")
        .size(760, 520)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(16.0)
                    .gap(12.0)
                    .background(BG),
            );

            tree.add_child(
                root,
                TextWidget::new("Drag the dividers to resize")
                    .font_size(16.0)
                    .color(HEADING),
            );

            // A frame that gives the split a defined box to fill.
            let frame = tree.add_child(
                root,
                Container::column()
                    .width_full()
                    .grow(1.0)
                    .radius(10.0)
                    .overflow_hidden()
                    .background(Color::rgb(0.08, 0.09, 0.12)),
            );

            // Outer horizontal split: sidebar | main.
            let (sidebar, main) = SplitPane::horizontal()
                .ratio(0.28)
                .min_ratio(0.15)
                .build(&mut tree, frame);

            panel(
                &mut tree,
                sidebar,
                SIDEBAR,
                "Sidebar",
                "A narrow pane. Drag the divider on the right to widen or shrink it — \
                 the content reflows and anything past the edge is clipped, not spilled.",
            );

            // Inner vertical split inside the main pane: editor over preview.
            let (editor, preview) = SplitPane::vertical().ratio(0.55).build(&mut tree, main);

            panel(
                &mut tree,
                editor,
                EDITOR,
                "Editor",
                "The top region of a nested vertical split. Drag the horizontal divider \
                 below to trade space with the preview.",
            );
            panel(
                &mut tree,
                preview,
                PREVIEW,
                "Preview",
                "The bottom region. Both dividers work independently, and the drag keeps \
                 tracking even if the cursor slides off the thin handle.",
            );

            tree
        });
}
