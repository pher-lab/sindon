//! progress_demo — Tier 2 progress indicators.
//!
//! Exercises the `ProgressBar` (determinate + indeterminate) and `Spinner`
//! widgets that promote knot's hand-built loading affordances into the
//! framework. The live-animated ones (indeterminate bar, spinners, and the
//! looping determinate bar) run off the shared animation clock so the
//! frame-vote pump can be eyeballed.
//!
//! Visual check:
//! - the static bars fill to 0 / 30 / 60 / 100% left-to-right;
//! - the "Looping" bar advances smoothly and wraps;
//! - the indeterminate bar's segment sweeps left→right inside the groove;
//! - each spinner's bright head rotates with a fading comet tail, and stays a
//!   circle (not an ellipse) at every size; the colored one recolors cleanly.

use std::time::Duration;

use sindon::app::App;
use sindon::core::Color;
use sindon::reactive::Signal;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, ProgressBar, Spinner, TextWidget};

const HEADING: Color = Color::rgb(0.92, 0.94, 1.0);
const MUTED: Color = Color::rgb(0.7, 0.72, 0.8);
const CARD: Color = Color::rgb(0.14, 0.15, 0.20);
const BG: Color = Color::rgb(0.10, 0.11, 0.15);

fn heading(text: &str) -> TextWidget {
    TextWidget::new(text).font_size(24.0).color(HEADING)
}

fn main() {
    // A signal the frame hook advances 0→1 in a loop, to drive the determinate
    // bar without any per-widget wiring.
    let looping = Signal::new(0.0f32);

    App::new()
        .title("sindon — progress indicators (Tier 2)")
        .size(560, 760)
        .tick_interval(Duration::from_millis(16))
        .run(move |scope| {
            let advance = looping;
            scope.on_frame(move |_ctx| {
                advance.set((advance.get() + 0.004).rem_euclid(1.0));
            });

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(28.0)
                    .gap(20.0)
                    .background(BG),
            );

            // ── ProgressBar (determinate) ─────────────────────────────────
            tree.add_child(root, heading("ProgressBar — determinate"));
            let bars = tree.add_child(
                root,
                Container::column()
                    .gap(16.0)
                    .padding(20.0)
                    .radius(10.0)
                    .width_full()
                    .background(CARD),
            );
            for (label, value) in [("0%", 0.0f32), ("30%", 0.3), ("60%", 0.6), ("100%", 1.0)] {
                let row =
                    tree.add_child(bars, Container::row().gap(12.0).align_center().width_full());
                tree.add_child(row, TextWidget::new(label).font_size(13.0).color(MUTED));
                tree.add_child(row, ProgressBar::new(value).label("Static progress"));
            }
            tree.add_child(
                bars,
                ProgressBar::new(looping).thickness(10.0).label("Looping"),
            );

            // ── ProgressBar (indeterminate) ───────────────────────────────
            tree.add_child(root, heading("ProgressBar — indeterminate"));
            let indet = tree.add_child(
                root,
                Container::column()
                    .gap(16.0)
                    .padding(20.0)
                    .radius(10.0)
                    .width_full()
                    .background(CARD),
            );
            tree.add_child(indet, ProgressBar::indeterminate().label("Working"));
            tree.add_child(
                indet,
                ProgressBar::indeterminate()
                    .thickness(10.0)
                    .fill_color(Color::rgb(0.3, 0.8, 0.5)),
            );

            // ── Spinner ───────────────────────────────────────────────────
            tree.add_child(root, heading("Spinner"));
            let spinners = tree.add_child(
                root,
                Container::row()
                    .gap(32.0)
                    .padding(20.0)
                    .radius(10.0)
                    .align_center()
                    .background(CARD),
            );
            tree.add_child(spinners, Spinner::new().size(16.0));
            tree.add_child(spinners, Spinner::new().size(24.0).label("Loading"));
            tree.add_child(spinners, Spinner::new().size(48.0));
            tree.add_child(
                spinners,
                Spinner::new().size(48.0).color(Color::rgb(0.95, 0.6, 0.3)),
            );

            tree
        });
}
