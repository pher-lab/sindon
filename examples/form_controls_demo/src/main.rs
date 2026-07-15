//! form_controls_demo — Phase 44 Tier 1 form controls.
//!
//! A settings-shaped screen exercising the non-text form widgets that grew out
//! of shroud's `Checkbox`-only inventory: `Switch` (and, as later commits land,
//! `Slider`, `Segmented`, `RadioGroup`). Each is shown bound to a `Signal` so
//! the two-way binding and the disabled variants can be eyeballed live.
//!
//! Visual check (Switch): the resting knob sits at the correct end; clicking or
//! Space (after Tab) slides it across and cross-fades the track; the two
//! "Linked" switches share one signal, so toggling either slides both; the
//! disabled pair stays dim and inert.

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, Switch, TextWidget};

fn main() {
    App::new()
        .title("shroud — form controls (Phase 44)")
        .size(560, 560)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(28.0)
                    .gap(18.0)
                    .background(Color::rgb(0.10, 0.11, 0.15)),
            );

            tree.add_child(
                root,
                TextWidget::new("Switch")
                    .font_size(24.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            // A signal shared by two switches — toggling either must slide both,
            // exercising the external-set-animates-in-paint path.
            let linked = Signal::new(true);

            let rows: Vec<Switch> = vec![
                Switch::new().label("Off by default"),
                Switch::new().on(true).label("On by default"),
                Switch::new().bind(linked).label("Linked A"),
                Switch::new().bind(linked).label("Linked B"),
                Switch::new().on(true).disabled(true).label("Disabled (on)"),
                Switch::new().disabled(true).label("Disabled (off)"),
            ];

            let list = tree.add_child(
                root,
                Container::column()
                    .gap(14.0)
                    .padding(16.0)
                    .radius(10.0)
                    .background(Color::rgb(0.14, 0.15, 0.20)),
            );
            for sw in rows {
                tree.add_child(list, sw);
            }

            tree
        });
}
