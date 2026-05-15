//! modal_demo — exercise the Phase 21 overlay layer primitive.
//!
//! Shows two flows on a single screen:
//! 1. **Open dialog** opens a centered modal (semi-transparent scrim,
//!    dismiss on outside-click and Escape) with a Cancel/Confirm pair.
//!    Both buttons pop the layer programmatically.
//! 2. **Last action** label below the button records the most recent
//!    result (cancelled / confirmed / opened / dismissed) so visual
//!    inspection can verify each dismiss path fires correctly.

use shroud::app::App;
use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

fn main() {
    App::new()
        .title("shroud — modal layer demo")
        .size(800, 600)
        .run(|_scope| {
            let last_action = Signal::new(String::from("(none yet)"));

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width(800.0)
                    .height(600.0)
                    .gap(16.0)
                    .center()
                    .background(Color::rgb(0.07, 0.07, 0.10)),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 21 — Modal layer primitive")
                    .font_size(28.0)
                    .color(Color::rgb(0.85, 0.88, 0.95)),
            );

            // Open-dialog button: pushes a modal via EventContext. The
            // build closure adds the modal's children after the framework
            // installs the layer root.
            let open_action = last_action;
            tree.add_child(
                root,
                Button::new("Open dialog")
                    .background(Color::rgb(0.30, 0.45, 0.85))
                    .radius(6.0)
                    .on_click(move |ctx| {
                        open_action.set("dialog opened".into());
                        ctx.push_layer(
                            LayerOptions::modal(),
                            Container::column()
                                .width(360.0)
                                .padding(24.0)
                                .gap(16.0)
                                .background(Color::rgb(0.14, 0.14, 0.18))
                                .radius(12.0),
                            move |tree, dialog| {
                                tree.add_child(
                                    dialog,
                                    TextWidget::new("Confirm action")
                                        .font_size(20.0)
                                        .color(Color::rgb(0.95, 0.95, 1.0)),
                                );
                                tree.add_child(
                                    dialog,
                                    TextWidget::new(
                                        "Outside-click and Escape both dismiss this dialog.",
                                    )
                                    .font_size(14.0)
                                    .color(Color::rgb(0.7, 0.72, 0.78)),
                                );

                                let buttons = tree
                                    .add_child(dialog, Container::row().gap(12.0).justify_center());
                                tree.add_child(
                                    buttons,
                                    Button::new("Cancel")
                                        .background(Color::rgb(0.35, 0.35, 0.40))
                                        .radius(6.0)
                                        .on_click(move |ctx| {
                                            open_action.set("cancelled".into());
                                            ctx.pop_top_layer();
                                        }),
                                );
                                tree.add_child(
                                    buttons,
                                    Button::new("Confirm")
                                        .background(Color::rgb(0.35, 0.65, 0.45))
                                        .radius(6.0)
                                        .on_click(move |ctx| {
                                            open_action.set("confirmed".into());
                                            ctx.pop_top_layer();
                                        }),
                                );
                            },
                        );
                    }),
            );

            // Status line. Reads `last_action` on every paint via the
            // reactive closure, so any handler that calls `set` flips
            // the text immediately. Note: outside-click and Escape paths
            // currently do not run any caller hook (Phase 21 does not
            // expose `on_dismiss` yet), so those dismissals leave the
            // label at whichever value the open / cancel / confirm
            // handler last wrote.
            tree.add_child(
                root,
                TextWidget::reactive(move || format!("Last action: {}", last_action.get_clone()))
                    .font_size(14.0)
                    .color(Color::rgb(0.55, 0.6, 0.7)),
            );

            tree
        });
}
