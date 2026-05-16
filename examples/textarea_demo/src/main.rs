//! textarea_demo — exercise the Phase 25 A-2 multi-line Input.
//!
//! Two textareas sharing the same screen:
//!
//! 1. **Free-form notes** — `Input::new().multiline().lines(4)` bound to
//!    a `Signal<String>`. Type any text; Enter inserts a newline,
//!    ArrowUp/Down move between hard lines preserving the visual column,
//!    Tab moves focus to the next textarea (focus manager owns Tab —
//!    the widget never sees it).
//!
//! 2. **Mnemonic-style** — a wider, narrower-text-width textarea showing
//!    a 12-word BIP39 mnemonic. Soft wrap should kick in when the words
//!    run past the right edge; the cursor still tracks per visual line.
//!
//! Live readout below echoes the bound signal so you can confirm Enter
//! and typing both round-trip through the binding.

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, Input, TextWidget};

fn main() {
    App::new()
        .title("shroud \u{2014} textarea demo (Phase 25)")
        .size(720, 640)
        .run(|_scope| {
            let notes = Signal::new(String::from("First line\nSecond line"));
            let mnemonic = Signal::new(String::new());

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
                TextWidget::new("Phase 25 \u{2014} multi-line Input")
                    .font_size(22.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            // ── Free-form notes ───────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Notes (Enter = newline, Tab moves focus)")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.70, 0.78)),
            );
            tree.add_child(
                root,
                Input::new()
                    .multiline()
                    .lines(5)
                    .value(notes)
                    .placeholder("Type here\u{2026}"),
            );

            // ── Mnemonic-style input ─────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("BIP39 mnemonic (soft-wrap test)")
                    .font_size(14.0)
                    .color(Color::rgb(0.65, 0.70, 0.78)),
            );
            tree.add_child(
                root,
                Input::new()
                    .multiline()
                    .lines(3)
                    .value(mnemonic)
                    .placeholder(
                        "Paste 12 words separated by spaces, e.g. \
                         abandon ability able about above absent absorb \
                         abstract absurd abuse access accident",
                    ),
            );

            // ── Live readouts ────────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Live bound value (notes):")
                    .font_size(13.0)
                    .color(Color::rgb(0.5, 0.55, 0.65)),
            );
            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    // Show \n as a visible glyph so the reader can
                    // confirm Enter actually inserted a newline rather
                    // than firing on_submit.
                    notes.get_clone().replace('\n', "\u{21B5} ")
                })
                .font_size(13.0)
                .color(Color::rgb(0.75, 0.85, 0.95)),
            );

            tree
        });
}
