//! theme_swap_demo — Phase 30: live theme swap via `App::theme(Reactive<Theme>)`.
//!
//! Demonstrates the two patterns Knot's settings UI needs:
//!
//! - **User toggle** — three buttons pick `ThemeChoice::Light`,
//!   `::Dark`, or `::System` via a `Signal<ThemeChoice>`.
//! - **OS-driven resolution** — when `System` is selected, the
//!   resolved theme follows `system_theme_signal()` so that toggling
//!   Windows / macOS appearance live-updates this window's colors
//!   without any per-widget rewiring.
//!
//! Both feed a single `Reactive::derive` handed to `App::theme(...)`.
//! The event loop re-evaluates it on every paint, so a `signal.set()`
//! followed by the redraw any click already triggers is enough — no
//! `tree.replace_root` or rebuild needed. Widgets that read theme
//! tokens during paint (Button background, Input border, focus ring,
//! the window clear color) flip in lockstep.

use shroud::app::{App, system_theme_signal};
use shroud::core::Theme;
use shroud::platform::SystemTheme;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    Light,
    Dark,
    System,
}

fn main() {
    // Both signals exist before `App::new` — the OS one comes from the
    // thread-local accessor, the user one is just `Signal::new`.
    // Folding them into a single `Reactive<Theme>` is the whole point:
    // the event loop pulls it on every paint, so updates are picked up
    // automatically once any redraw fires.
    let user_choice: Signal<ThemeChoice> = Signal::new(ThemeChoice::System);
    let os_theme = system_theme_signal();

    let theme_reactive = Reactive::derive(move || match user_choice.get() {
        ThemeChoice::Light => Theme::light(),
        ThemeChoice::Dark => Theme::dark(),
        // None covers Linux outside GNOME/KDE — fall back to dark, the
        // historical shroud default. Apps that ship a different
        // preferred fallback can swap it here.
        ThemeChoice::System => match os_theme.get() {
            Some(SystemTheme::Light) => Theme::light(),
            Some(SystemTheme::Dark) | None => Theme::dark(),
        },
    });

    App::new()
        .title("shroud \u{2014} theme swap demo (Phase 30)")
        .size(640, 360)
        .theme(theme_reactive)
        .run(move |_scope| {
            let mut tree = WidgetTree::new();
            // No `.background(...)` override on root — let the
            // window's clear color (driven by `theme.colors.background`
            // pulled in `Renderer::render`) handle the canvas itself.
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .gap(16.0),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 30 \u{2014} live theme swap")
                    .font_size(22.0),
            );

            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    let choice_label = match user_choice.get() {
                        ThemeChoice::Light => "Light",
                        ThemeChoice::Dark => "Dark",
                        ThemeChoice::System => "System",
                    };
                    let os_label = match os_theme.get() {
                        Some(SystemTheme::Light) => "OS: Light",
                        Some(SystemTheme::Dark) => "OS: Dark",
                        None => "OS: (not reported)",
                    };
                    format!("Choice: {choice_label}  \u{2022}  {os_label}")
                })
                .font_size(14.0),
            );

            let buttons_row = tree.add_child(
                root,
                Container::row().gap(12.0),
            );

            for (label, choice) in [
                ("Light", ThemeChoice::Light),
                ("Dark", ThemeChoice::Dark),
                ("System", ThemeChoice::System),
            ] {
                tree.add_child(
                    buttons_row,
                    Button::new(label).radius(6.0).on_click(move |_ctx| {
                        user_choice.set(choice);
                    }),
                );
            }

            tree.add_child(
                root,
                TextWidget::new(
                    "Click a button to flip the in-app choice. With System selected, \
                     toggle the OS dark/light setting and the window repaints on the \
                     ThemeChanged event \u{2014} no app code runs between the OS event \
                     and the next frame.",
                )
                .font_size(12.0),
            );

            tree
        });
}
