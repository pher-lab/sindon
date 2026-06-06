//! animation_demo — exercise the B-8 animation / transition primitive.
//!
//! Clicking **Toggle** flips a `Signal<bool>` and retargets several
//! `Animated<Color>` values. Each one tweens from its current value to the
//! new target over a fixed duration; the widgets read the eased value every
//! paint through the `Animated<T>: Into<Reactive<T>>` conversion, and the
//! event loop keeps redrawing only while something is still moving.
//!
//! Three things are on display:
//! 1. **Easing comparison** — three swatches share one toggle but use
//!    `Linear` / `EaseOut` / `EaseInOut`, so the same color transition runs
//!    at visibly different rates.
//! 2. **Opacity fade** — a box tweens its background *alpha* between opaque
//!    and near-transparent, the building block for a theme-switch fade.
//! 3. **Hover fade** — hoverable containers and buttons ease between their
//!    resting and hover colors by default (the B-8 wiring); one row opts out
//!    with `hover_transition(Duration::ZERO)` to contrast the instant flip.
//! 4. **Transform rotate** — a disclosure header whose `▸` chevron eases
//!    0° → 90° as the section opens, driven by `TextWidget::rotation` reading
//!    an `Animated<f32>`. This is the third and final B-8 wiring path.
//!
//! Visual check: click Toggle and watch the swatches cross-fade at different
//! rates and the box fade out/in; hover the bottom rows / button and watch
//! the highlight ease in and out; click the Details header and watch the
//! chevron rotate as the body appears. When motion stops, CPU goes idle (no
//! continuous repaint at rest).

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
        .size(680, 720)
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

            // Hover fade (B-8 wiring): hoverable containers and buttons now
            // ease between their resting and hover colors by default — move
            // the cursor over the rows below to see it. The right-hand row
            // opts out with `hover_transition(Duration::ZERO)` so the default
            // fade and the old instant flip sit side by side.
            tree.add_child(
                root,
                TextWidget::new("Hover fade (B-8 wiring) — hover the rows / button")
                    .color(Color::rgb(0.70, 0.70, 0.76)),
            );
            let hover_row = tree.add_child(root, Container::row().gap(16.0).height(60.0));
            let surface = Color::rgb(0.16, 0.17, 0.21);
            let surface_hover = Color::rgb(0.27, 0.30, 0.38);
            tree.add_child(
                hover_row,
                Container::row()
                    .grow(1.0)
                    .height_full()
                    .align_center()
                    .padding(16.0)
                    .radius(10.0)
                    .background(surface)
                    .hover_background(surface_hover),
            );
            tree.add_child(
                hover_row,
                Container::row()
                    .grow(1.0)
                    .height_full()
                    .align_center()
                    .padding(16.0)
                    .radius(10.0)
                    .background(surface)
                    .hover_background(surface_hover)
                    .hover_transition(Duration::ZERO),
            );
            tree.add_child(
                root,
                Button::new("Hover me")
                    .radius(8.0)
                    .background(Color::rgb(0.20, 0.45, 0.85))
                    .hover_background(Color::rgb(0.35, 0.60, 0.95))
                    .on_click(|_| {}),
            );

            // Transform rotate (B-8 wiring 3/3): a disclosure header whose
            // chevron eases 0° → 90° as the section opens. The chevron is just
            // a `TextWidget` glyph with `.rotation(Animated<f32>)`; the
            // renderer spins the glyph quad about its center (rects stay
            // axis-aligned), and `.visible` cascades the body in / out.
            tree.add_child(
                root,
                TextWidget::new("Transform rotate (B-8 wiring 3/3) — click the header")
                    .color(Color::rgb(0.70, 0.70, 0.76)),
            );
            let open = Signal::new(false);
            let chevron = Animated::new(0.0_f32, DURATION, Easing::EaseInOut);
            let chevron_toggle = chevron.clone();
            let header = tree.add_child(
                root,
                Container::row()
                    .gap(10.0)
                    .align_center()
                    .padding(10.0)
                    .radius(8.0)
                    .hoverable()
                    .on_press(move |_pos, _ctx| {
                        let next = !open.get();
                        open.set(next);
                        chevron_toggle.set(if next { 90.0 } else { 0.0 });
                    }),
            );
            tree.add_child(
                header,
                TextWidget::new("\u{25B8}")
                    .font_size(20.0)
                    .color(Color::WHITE)
                    .rotation(chevron),
            );
            tree.add_child(header, TextWidget::new("Details").color(Color::WHITE));

            let body = tree.add_child(
                root,
                Container::column()
                    .visible(open)
                    .gap(6.0)
                    .padding(12.0)
                    .radius(8.0)
                    .background(Color::rgb(0.16, 0.17, 0.21)),
            );
            tree.add_child(
                body,
                TextWidget::new("The chevron eases 0° → 90° as this section opens.")
                    .color(Color::rgb(0.70, 0.70, 0.76)),
            );
            tree.add_child(
                body,
                TextWidget::new("Rotation is per-glyph in the renderer; backgrounds stay square.")
                    .color(Color::rgb(0.70, 0.70, 0.76)),
            );

            tree
        });
}
