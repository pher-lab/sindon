//! animation_demo — exercise the B-8 animation / transition primitive.
//!
//! Clicking **Toggle** flips a `Signal<bool>` and retargets several
//! `Animated<Color>` values. Each one tweens from its current value to the
//! new target over a fixed duration; the widgets read the eased value every
//! paint through the `Animated<T>: Into<Reactive<T>>` conversion, and the
//! event loop keeps redrawing only while something is still moving.
//!
//! Two things are on display:
//! 1. **Easing comparison** — three swatches share one toggle but use
//!    `Linear` / `EaseOut` / `EaseInOut`, so the same color transition runs
//!    at visibly different rates.
//! 2. **Opacity fade** — a box tweens its background *alpha* between opaque
//!    and near-transparent, the building block for a theme-switch fade.
//!
//! Visual check: click Toggle and watch the swatches cross-fade at different
//! rates and the bottom box fade out/in. When motion stops, CPU goes idle
//! (no continuous repaint at rest).

use std::time::Duration;

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::{Animated, Easing, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

/// Transition length for every animation in the demo.
const DURATION: Duration = Duration::from_millis(450);

fn main() {
    App::new()
        .title("shroud — animation / transition demo")
        .size(680, 560)
        .run(|_scope| {
            // The two endpoints every swatch tweens between.
            let color_a = Color::rgb(0.20, 0.45, 0.85);
            let color_b = Color::rgb(0.90, 0.40, 0.30);

            // Same transition, three different easing curves.
            let linear = Animated::new(color_a, DURATION, Easing::Linear);
            let ease_out = Animated::new(color_a, DURATION, Easing::EaseOut);
            let ease_in_out = Animated::new(color_a, DURATION, Easing::EaseInOut);

            // A box whose background alpha fades between opaque and faint.
            let fade = Animated::new(color_b, DURATION, Easing::EaseInOut);

            // Toggle state, surfaced as a label and driving the targets.
            let on = Signal::new(false);

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .gap(20.0)
                    .padding(28.0)
                    .background(Color::rgb(0.10, 0.11, 0.14)),
            );

            tree.add_child(
                root,
                TextWidget::new("Animation / transition (B-8)")
                    .font_size(24.0)
                    .color(Color::WHITE),
            );
            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    format!("State: {}", if on.get() { "B" } else { "A" })
                })
                .color(Color::rgb(0.70, 0.70, 0.76)),
            );

            // Toggle button: flip the state and retarget every animation. The
            // `Animated` handles are `Rc`-backed, so cloning them into the
            // closure shares the same underlying value the swatches read.
            let toggle_linear = linear.clone();
            let toggle_ease_out = ease_out.clone();
            let toggle_ease_in_out = ease_in_out.clone();
            let toggle_fade = fade.clone();
            tree.add_child(
                root,
                Button::new("Toggle").radius(8.0).on_click(move |_ctx| {
                    let next = !on.get();
                    on.set(next);
                    let target = if next { color_b } else { color_a };
                    toggle_linear.set(target);
                    toggle_ease_out.set(target);
                    toggle_ease_in_out.set(target);
                    // Fade box: keep the hue, animate alpha to near-zero on B.
                    toggle_fade.set(if next {
                        Color::rgba(color_b.r, color_b.g, color_b.b, 0.12)
                    } else {
                        color_b
                    });
                }),
            );

            // Easing-comparison row of three labelled swatches.
            let row = tree.add_child(root, Container::row().gap(16.0).height(150.0));
            for (label, anim) in [
                ("Linear", linear),
                ("EaseOut", ease_out),
                ("EaseInOut", ease_in_out),
            ] {
                let col = tree.add_child(row, Container::column().grow(1.0).gap(8.0));
                tree.add_child(
                    col,
                    Container::column()
                        .grow(1.0)
                        .width_full()
                        .radius(12.0)
                        .background(anim),
                );
                tree.add_child(
                    col,
                    TextWidget::new(label).color(Color::rgb(0.70, 0.70, 0.76)),
                );
            }

            // Opacity fade box.
            tree.add_child(
                root,
                TextWidget::new("Opacity fade (alpha tween over the dark backdrop)")
                    .color(Color::rgb(0.70, 0.70, 0.76)),
            );
            tree.add_child(
                root,
                Container::column()
                    .height(100.0)
                    .width_full()
                    .radius(12.0)
                    .background(fade),
            );

            tree
        });
}
