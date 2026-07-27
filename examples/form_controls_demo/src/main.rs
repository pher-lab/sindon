//! form_controls_demo — Phase 44 Tier 1 form controls.
//!
//! A settings-shaped screen exercising the non-text form widgets that grew out
//! of sindon's `Checkbox`-only inventory: `Switch` (and, as later commits land,
//! `Slider`, `Segmented`, `RadioGroup`). Each is shown bound to a `Signal` so
//! the two-way binding and the disabled variants can be eyeballed live.
//!
//! Visual check (Switch): the resting knob sits at the correct end; clicking or
//! Space (after Tab) slides it across and cross-fades the track; the two
//! "Linked" switches share one signal, so toggling either slides both; the
//! disabled pair stays dim and inert.

use sindon::app::App;
use sindon::core::Color;
use sindon::reactive::Signal;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, RadioGroup, Segmented, Slider, Switch, TextWidget};

fn main() {
    App::new()
        .title("sindon — form controls (Phase 44)")
        .size(600, 980)
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

            // ── Slider ────────────────────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Slider")
                    .font_size(24.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            // Two sliders share one signal — dragging either moves both.
            let volume = Signal::new(40.0);
            let sliders = tree.add_child(
                root,
                Container::column()
                    .gap(18.0)
                    .padding(16.0)
                    .radius(10.0)
                    .background(Color::rgb(0.14, 0.15, 0.20)),
            );
            tree.add_child(sliders, Slider::new(0.0, 100.0).bind(volume));
            tree.add_child(sliders, Slider::new(0.0, 100.0).bind(volume));
            tree.add_child(sliders, Slider::new(0.0, 10.0).step(1.0).value(3.0));
            tree.add_child(sliders, Slider::new(0.0, 100.0).value(60.0).disabled(true));

            // ── Segmented ─────────────────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("Segmented")
                    .font_size(24.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            let view = Signal::new(0usize);
            let segs = tree.add_child(
                root,
                Container::column()
                    .gap(14.0)
                    .padding(16.0)
                    .radius(10.0)
                    .background(Color::rgb(0.14, 0.15, 0.20)),
            );
            tree.add_child(segs, Segmented::new(["Edit", "Preview"]).bind(view));
            tree.add_child(
                segs,
                Segmented::new(["Day", "Week", "Month", "Year"]).selected(1),
            );
            tree.add_child(
                segs,
                Segmented::new(["Low", "Med", "High"])
                    .selected(2)
                    .disabled(true),
            );

            // ── RadioGroup ────────────────────────────────────────────────
            tree.add_child(
                root,
                TextWidget::new("RadioGroup")
                    .font_size(24.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            let theme_choice = Signal::new(0usize);
            let radios = tree.add_child(
                root,
                Container::row()
                    .gap(32.0)
                    .padding(16.0)
                    .radius(10.0)
                    .align_center()
                    .background(Color::rgb(0.14, 0.15, 0.20)),
            );
            tree.add_child(
                radios,
                RadioGroup::new(["System", "Light", "Dark"]).bind(theme_choice),
            );
            tree.add_child(
                radios,
                RadioGroup::new(["On", "Off"]).selected(1).disabled(true),
            );

            tree
        });
}
