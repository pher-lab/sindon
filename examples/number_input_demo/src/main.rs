//! number_input_demo — exercise the Phase 28 A-3 numeric Input mode.
//!
//! Mirrors Knot's `BackupSettingsModal` "interval days" + "retention count"
//! pair: two numeric `Input`s bound to `Signal<i64>`, each with its own
//! min / max range. Live readouts below echo the bound signals so the
//! user can watch the clamp behavior:
//!
//! - Letters / punctuation are dropped silently while typing.
//! - Typing past `max_value` clamps the **signal** immediately (the
//!   buffer keeps showing the typed digits until you Tab away).
//! - On focus loss the buffer snaps to the canonical decimal form of
//!   the signal (e.g. typed "999" with max 365 → signal becomes 365,
//!   buffer redraws as "365" once focus moves).
//! - Clearing the buffer entirely leaves the signal at its last valid
//!   value; focus loss re-renders that value back into the field.

use sindon::app::App;
use sindon::core::Color;
use sindon::reactive::Signal;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, Input, TextWidget};

fn main() {
    App::new()
        .title("sindon \u{2014} number input demo (Phase 28)")
        .size(640, 520)
        .run(|_scope| {
            let interval_days = Signal::new(7i64);
            let retention_count = Signal::new(10i64);

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
                TextWidget::new("Phase 28 \u{2014} numeric Input")
                    .font_size(22.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            tree.add_child(
                root,
                TextWidget::new(
                    "Type letters \u{2014} dropped. Type past the max \u{2014} signal \
                     clamps immediately, buffer snaps on Tab.",
                )
                .font_size(13.0)
                .color(Color::rgb(0.65, 0.70, 0.78)),
            );

            // ── Interval days (1..=365) ───────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Backup interval (days, 1\u{2013}365)")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.70, 0.78)),
            );
            tree.add_child(
                root,
                Input::new()
                    .min_value(1)
                    .max_value(365)
                    .number_value(interval_days),
            );

            // ── Retention count (1..=1000) ────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Retention count (1\u{2013}1000)")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.70, 0.78)),
            );
            tree.add_child(
                root,
                Input::new()
                    .min_value(1)
                    .max_value(1000)
                    .number_value(retention_count),
            );

            // ── Live readouts ────────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Bound Signal<i64> values:")
                    .font_size(13.0)
                    .color(Color::rgb(0.5, 0.55, 0.65)),
            );
            {
                let s = interval_days;
                tree.add_child(
                    root,
                    TextWidget::reactive(move || format!("interval_days = {}", s.get()))
                        .font_size(14.0)
                        .color(Color::rgb(0.75, 0.85, 0.95)),
                );
            }
            {
                let s = retention_count;
                tree.add_child(
                    root,
                    TextWidget::reactive(move || format!("retention_count = {}", s.get()))
                        .font_size(14.0)
                        .color(Color::rgb(0.75, 0.85, 0.95)),
                );
            }

            tree
        });
}
