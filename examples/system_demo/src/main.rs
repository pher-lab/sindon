//! system_demo — Phase 29 A-13/A-14: surface OS theme + locale to apps.
//!
//! Mirrors the Knot `theme = "system"` / `language = "system"` pair:
//!
//! - `AppScope::system_theme()` returns a `Signal<Option<SystemTheme>>`
//!   that the event loop keeps in sync with `WindowEvent::ThemeChanged`.
//!   The text below subscribes via `TextWidget::reactive` — toggle your
//!   OS appearance (Windows: Settings → Personalization → Colors;
//!   macOS: System Settings → Appearance) and the readout updates
//!   without restarting the app.
//! - `system_locale()` returns the BCP-47 tag once at startup; no live
//!   updates because no platform reports locale changes mid-process.
//!
//! Resolving these into the actual visual theme / shipped language is
//! up to the app — shroud's `App::theme(...)` is still set at build
//! time. The detection signal is what makes "system" mode possible.

use shroud::app::App;
use shroud::core::Color;
use shroud::platform::SystemTheme;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, TextWidget};

fn main() {
    App::new()
        .title("shroud \u{2014} system theme/locale demo (Phase 29)")
        .size(600, 320)
        .run(|scope| {
            let os_theme = scope.system_theme();
            let locale = scope
                .system_locale()
                .unwrap_or_else(|| "<unknown>".to_string());

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .gap(14.0)
                    .background(Color::rgb(0.08, 0.08, 0.12)),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 29 \u{2014} system detection")
                    .font_size(22.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );

            tree.add_child(
                root,
                TextWidget::new(
                    "Toggle your OS light/dark setting \u{2014} the theme line below \
                     refreshes via the reactive Signal exposed by AppScope.",
                )
                .font_size(13.0)
                .color(Color::rgb(0.65, 0.70, 0.78)),
            );

            tree.add_child(
                root,
                TextWidget::reactive(move || match os_theme.get() {
                    Some(SystemTheme::Light) => "OS theme: Light".to_string(),
                    Some(SystemTheme::Dark) => "OS theme: Dark".to_string(),
                    None => "OS theme: (not reported by platform)".to_string(),
                })
                .font_size(16.0)
                .color(Color::rgb(0.75, 0.85, 0.95)),
            );

            tree.add_child(
                root,
                TextWidget::new(format!("OS locale: {locale}"))
                    .font_size(16.0)
                    .color(Color::rgb(0.75, 0.85, 0.95)),
            );

            tree.add_child(
                root,
                TextWidget::new(
                    "(Locale is a one-shot read at startup. No supported platform fires \
                     events for runtime locale changes, so we don't either.)",
                )
                .font_size(12.0)
                .color(Color::rgb(0.45, 0.50, 0.58)),
            );

            tree
        });
}
