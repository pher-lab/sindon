//! toast_demo — Tier 2 transient status toasts.
//!
//! Buttons push toasts of each severity; the `toast` overlay (mounted once with
//! `toast::mount`) stacks them bottom-center, fades them in, holds, and fades
//! them out on their own. The app underneath stays fully interactive the whole
//! time — the overlay is click-through, so you can keep clicking buttons while
//! toasts are on screen.
//!
//! Visual check:
//! - each button spawns a toast at the bottom that fades in, holds ~4s, fades
//!   out;
//! - the accent stripe color matches the severity (info/success/warning/error);
//! - stacking multiple pushes them upward; a 6th drops the oldest;
//! - "Dismiss all" fades every visible toast out at once;
//! - buttons stay clickable while toasts are up (no input is stolen).

use sindon::app::App;
use sindon::core::Color;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget, toast};

const HEADING: Color = Color::rgb(0.92, 0.94, 1.0);
const BG: Color = Color::rgb(0.10, 0.11, 0.15);

fn main() {
    App::new()
        .title("sindon — toasts (Tier 2)")
        .size(520, 420)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(28.0)
                    .gap(20.0)
                    .background(BG),
            );

            tree.add_child(
                root,
                TextWidget::new("Toasts").font_size(24.0).color(HEADING),
            );
            tree.add_child(
                root,
                TextWidget::new(
                    "Each button spawns a self-dismissing toast. The app stays interactive.",
                )
                .font_size(14.0)
                .color(Color::rgb(0.7, 0.72, 0.8)),
            );

            let row = tree.add_child(root, Container::row().gap(12.0));
            tree.add_child(
                row,
                Button::new("Info").on_click(|_| {
                    toast::info("Synced 3 notes.");
                }),
            );
            tree.add_child(
                row,
                Button::new("Success").on_click(|_| {
                    toast::success("Vault unlocked.");
                }),
            );
            tree.add_child(
                row,
                Button::new("Warning").on_click(|_| {
                    toast::warning("Auto-lock in 30 seconds.");
                }),
            );
            tree.add_child(
                row,
                Button::new("Error").on_click(|_| {
                    toast::error("Export failed: disk full. Free some space and try again.");
                }),
            );

            let row2 = tree.add_child(root, Container::row().gap(12.0));
            tree.add_child(
                row2,
                Button::new("Burst ×6").on_click(|_| {
                    for i in 1..=6 {
                        toast::info(format!("Message {i} of 6"));
                    }
                }),
            );
            tree.add_child(
                row2,
                Button::new("Dismiss all").on_click(|_| {
                    toast::clear();
                }),
            );

            // Mount the toast overlay once, after the main tree is built.
            toast::mount(&mut tree);

            tree
        });
}
