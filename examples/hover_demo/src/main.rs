//! hover_demo — exercise the Phase 23 A-4 hover state primitive.
//!
//! Three patterns on a single screen:
//! 1. **Note-list rows** — Containers with `.hoverable()` that pick up
//!    `theme.hover.bg` when the cursor enters. Mirrors Knot's NoteItem.
//! 2. **Custom-tinted row** — `.hover_background(color)` for cases
//!    where the theme default doesn't match the row's resting bg.
//! 3. **Hoverable row that wraps a Button** — verifies the hover bubble
//!    fires the row's MouseEnter even when the deepest hit is the
//!    inner button.
//!
//! Visual check: move the mouse over each row; the row tints. Move into
//! the third row's button; both button (hover_background from theme
//! primary_hover) and row should be lit at once.

use sindon::app::App;
use sindon::core::Color;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget};

fn main() {
    App::new()
        .title("sindon — hover state demo")
        .size(640, 520)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .gap(16.0)
                    .background(Color::rgb(0.08, 0.08, 0.12)),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 23 \u{2014} hover state")
                    .font_size(24.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            // Pattern 1: theme-default hover rows. The list itself has no
            // background so each row reads as "lifted off surface" only
            // when actually hovered.
            tree.add_child(
                root,
                TextWidget::new("1. Theme-default rows (.hoverable())")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.7, 0.78)),
            );
            let list = tree.add_child(
                root,
                Container::column()
                    .padding(4.0)
                    .gap(2.0)
                    .background(Color::rgb(0.12, 0.12, 0.18))
                    .radius(6.0),
            );
            for title in [
                "Welcome to Knot",
                "Meeting notes \u{2014} Apr 30",
                "Grocery list",
            ] {
                let row = tree.add_child(
                    list,
                    Container::row()
                        .hoverable()
                        .padding(10.0)
                        .radius(4.0)
                        .align_center(),
                );
                tree.add_child(
                    row,
                    TextWidget::new(title).color(Color::rgb(0.85, 0.87, 0.92)),
                );
            }

            // Pattern 2: explicit hover bg override. Useful when the
            // row already has a bg (e.g., warning row) and the theme
            // default would clash.
            tree.add_child(
                root,
                TextWidget::new("2. Explicit .hover_background(\u{2026})")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.7, 0.78)),
            );
            let warn_row = tree.add_child(
                root,
                Container::row()
                    .padding(12.0)
                    .radius(6.0)
                    .background(Color::rgb(0.30, 0.20, 0.10))
                    .hover_background(Color::rgb(0.45, 0.30, 0.15))
                    .align_center(),
            );
            tree.add_child(
                warn_row,
                TextWidget::new("Backup is overdue \u{2014} hover me")
                    .color(Color::rgb(0.95, 0.85, 0.70)),
            );

            // Pattern 3: hoverable row wrapping a Button. Validates the
            // ancestor-bubble path in update_hover_in — without it the
            // row would never see MouseEnter because the Button is the
            // deepest hit.
            tree.add_child(
                root,
                TextWidget::new("3. Hoverable row containing a Button (bubble test)")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.7, 0.78)),
            );
            let inner_row = tree.add_child(
                root,
                Container::row()
                    .hoverable()
                    .padding(10.0)
                    .gap(12.0)
                    .radius(4.0)
                    .align_center(),
            );
            tree.add_child(
                inner_row,
                TextWidget::new("Untitled note").color(Color::rgb(0.85, 0.87, 0.92)),
            );
            tree.add_child(
                inner_row,
                Button::new("Delete")
                    .background(Color::rgb(0.45, 0.20, 0.20))
                    .radius(4.0)
                    .on_click(|_| {}),
            );

            tree
        });
}
