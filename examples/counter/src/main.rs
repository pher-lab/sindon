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

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

fn main() {
    App::new()
        .title("shroud — counter")
        .size(400, 300)
        .run(|_handle| {
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

            // Increment button — `count` is `Copy`, so this move closure
            // captures a handle and can be called repeatedly. Background
            // flips with parity via the `Reactive::derive` above.
            tree.add_child(
                root,
                Button::new("Increment")
                    .background(button_bg)
                    .on_click(move || {
                        count.update(|n| *n += 1);
                    }),
            );

            tree
        });
}
