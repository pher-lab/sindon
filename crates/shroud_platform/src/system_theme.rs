//! OS-provided UI theme preference (light / dark).
//!
//! The OS exposes a coarse light/dark hint that apps can honor when
//! their own `theme = "system"` setting is chosen. Resolved by winit
//! and surfaced to apps two ways:
//!
//! - One-shot snapshot via [`PlatformWindow::system_theme`](crate::PlatformWindow::system_theme).
//!   `None` when the OS doesn't report a preference (X11 without a
//!   recognized desktop environment, headless contexts).
//! - Reactive subscription via `shroud_app::AppScope::system_theme`,
//!   which yields a `Signal<Option<SystemTheme>>` updated by
//!   `WindowEvent::ThemeChanged` for the life of the app.
//!
//! Choosing the theme itself (which colors to paint) is the app's
//! responsibility — shroud only reports what the OS prefers.

use winit::window::Theme as WinitTheme;

/// OS-reported UI theme preference. Mirrors winit's `Theme`, but kept
/// in `shroud_platform` so apps can match on it without importing
/// winit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemTheme {
    Light,
    Dark,
}

impl SystemTheme {
    /// Convert the winit-native enum into the shroud-facing one. Pure
    /// 1:1 mapping; kept as a method so the event loop has a single
    /// place to round-trip and so the conversion is unit-testable
    /// without winit boilerplate at the call site.
    pub fn from_winit(theme: WinitTheme) -> Self {
        match theme {
            WinitTheme::Light => Self::Light,
            WinitTheme::Dark => Self::Dark,
        }
    }
}
