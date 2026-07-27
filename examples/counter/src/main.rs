//! Reactive counter — the first end-to-end demo of `Signal` driving a widget.
//!
//! Clicking the button increments a `Signal<i32>`. Several widget attributes
//! read the signal on every paint, and the event loop requests a redraw after
//! every mouse event, so the UI updates:
//!
//! - `TextWidget::reactive` — label text reflects the count.
//! - Button `.background(...)` — flips between two colors on count parity,
//!   demonstrating `Reactive::derive` composing a `Signal` into a `Color`.
//! - Container `.background(...)` — tinted by the same parity, to show that
//!   the same pattern applies uniformly across widgets.
//! - Button `.visible(...)` (Phase 18b) — a "Reset" button only appears when
//!   `count > 0`, collapsing the slot in layout when hidden.
//! - Container `.visible(...)` (Phase 18b) — an "odd!" badge container appears
//!   on odd counts and disappears (with its child TextWidget via cascade) on
//!   even counts.

use sindon::app::App;
use sindon::core::Color;
use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget};

fn main() {
    App::new()
        .title("sindon — counter")
        .size(400, 300)
        .run(|_scope| {
            // A reactive signal. `Signal<T>` is `Copy`, so moving it into
            // multiple closures is just copying a handle — no reference
            // counting, no lifetimes to juggle.
            let count = Signal::new(0i32);

            // Parity-derived colors. `Reactive::derive` wraps a closure that
            // the widget re-runs on every paint; because `count` is `Copy`,
            // each derive owns its own handle to the same underlying signal.
            let even_tint = Color::rgb(0.18, 0.22, 0.32);
            let odd_tint = Color::rgb(0.32, 0.18, 0.22);
            let container_bg: Reactive<Color> = Reactive::derive(move || {
                if count.get() % 2 == 0 {
                    even_tint
                } else {
                    odd_tint
                }
            });

            let even_btn = Color::rgb(0.25, 0.55, 0.85);
            let odd_btn = Color::rgb(0.85, 0.45, 0.25);
            let button_bg: Reactive<Color> = Reactive::derive(move || {
                if count.get() % 2 == 0 {
                    even_btn
                } else {
                    odd_btn
                }
            });

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width(400.0)
                    .height(300.0)
                    .gap(16.0)
                    .center()
                    .background(container_bg),
            );

            // Reactive text — the closure runs on every paint, reading
            // the latest value of `count` each time. The widget reports
            // its natural width via `Widget::measure`, so `.center()`
            // positions it correctly on the cross axis without a wrapper.
            tree.add_child(
                root,
                TextWidget::reactive(move || format!("Count: {}", count.get())).font_size(32.0),
            );

            // Phase 18b — a small badge that only appears on odd counts.
            // Hidden via `Container::visible(Reactive<bool>)`; the child
            // TextWidget inherits the collapse via Taffy's `display: none`
            // cascade, so we only toggle the parent.
            let badge = tree.add_child(
                root,
                Container::column()
                    .padding(6.0)
                    .background(Color::rgb(0.85, 0.55, 0.25))
                    .visible(Reactive::derive(move || count.get() % 2 != 0)),
            );
            tree.add_child(
                badge,
                TextWidget::new("odd!").font_size(14.0).color(Color::BLACK),
            );

            // Increment button — `count` is `Copy`, so this move closure
            // captures a handle and can be called repeatedly. Background
            // flips with parity via the `Reactive::derive` above.
            tree.add_child(
                root,
                Button::new("Increment")
                    .background(button_bg)
                    .on_click(move |_ctx| {
                        count.update(|n| *n += 1);
                    }),
            );

            // Phase 18b — a Reset button that only appears when count > 0.
            // Driven by `Button::visible(Reactive<bool>)`; when hidden, its
            // slot collapses entirely (no layout space, no paint, no clicks).
            tree.add_child(
                root,
                Button::new("Reset")
                    .background(Color::rgb(0.5, 0.5, 0.5))
                    .visible(Reactive::derive(move || count.get() > 0))
                    .on_click(move |_ctx| count.set(0)),
            );

            tree
        });
}
