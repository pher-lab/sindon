use std::sync::Arc;

use crate::display_protection::{DisplayProtection, DisplayProtectionResult};
use crate::system_theme::SystemTheme;
#[cfg(target_os = "windows")]
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

/// Wraps a winit `Window` with platform-specific security extensions.
pub struct PlatformWindow {
    window: Arc<Window>,
    display_protection: DisplayProtection,
}

impl PlatformWindow {
    /// Create a new window on the given event loop.
    pub fn new(event_loop: &ActiveEventLoop, title: &str, width: u32, height: u32) -> Self {
        let attrs = WindowAttributes::default()
            .with_title(title)
            .with_inner_size(LogicalSize::new(width, height));

        let window = event_loop
            .create_window(attrs)
            .expect("failed to create window");

        let window = Arc::new(window);
        let display_protection = DisplayProtection::new(Arc::clone(&window));

        Self {
            window,
            display_protection,
        }
    }

    /// Get a clone of the `Arc<Window>` for sharing with the renderer.
    pub fn arc(&self) -> Arc<Window> {
        Arc::clone(&self.window)
    }

    /// Get the current inner size in physical pixels.
    pub fn inner_size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    /// Request a redraw.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Access the display protection manager.
    pub fn display_protection(&self) -> &DisplayProtection {
        &self.display_protection
    }

    /// Mutable access to the display protection manager.
    pub fn display_protection_mut(&mut self) -> &mut DisplayProtection {
        &mut self.display_protection
    }

    /// Enable screen capture prevention (convenience method).
    ///
    /// Delegates to `DisplayProtection::enable()`.
    pub fn set_capture_prevention(&mut self, enabled: bool) -> DisplayProtectionResult {
        if enabled {
            self.display_protection.enable()
        } else {
            self.display_protection.disable()
        }
    }

    /// Allow / forbid IME (Input Method Editor) input on this window.
    ///
    /// On Windows / macOS / X11, IME starts disabled by default; turning it
    /// on is what lets users type CJK (Japanese, Chinese, Korean) and other
    /// composed scripts via the OS-level IME — keystrokes get bundled into
    /// `WindowEvent::Ime(Ime::Commit(text))` after composition completes.
    /// Calling with `false` reverts to raw key events only.
    ///
    /// shroud enables IME globally on window create (see `event_loop::resumed`)
    /// so every `Input` / `SecureInput` accepts composed text without each
    /// widget having to opt in. Apps that need to disable IME for a sensitive
    /// flow (e.g. a numeric-only PIN entry) can call this with `false`
    /// directly on the platform window.
    ///
    /// IME open *status* (composing vs. direct alphanumeric) is deliberately
    /// left to the OS and the user. Enabling IME associates the input context
    /// but does not force composition mode on, so a Japanese-layout user keeps
    /// their last-used mode and toggles composing with 半角/全角 — matching
    /// native apps. An earlier build forced the IME open on every focus via
    /// `ImmSetOpenStatus(true)`, which wrongly defaulted every window focus to
    /// hiragana; that was removed once `App::exploit_mitigation` — the real
    /// cause of the "IME totally dead" symptom it was papering over — was
    /// defaulted off.
    ///
    /// Composition itself only works when `App::exploit_mitigation` (in
    /// `shroud_app`) is left at its default (`false`); turning that on
    /// activates `ProcessExtensionPointDisablePolicy`, which blocks the
    /// extension-DLL plumbing the IME relies on. The trade-off is documented
    /// on that builder.
    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    /// Anchor the OS IME composition / candidate window near the given
    /// rectangle (in logical window coordinates). Forwards directly to
    /// winit's [`Window::set_ime_cursor_area`].
    ///
    /// shroud calls this once per paint when a text widget is focused —
    /// the widget writes its caret rect into `PaintContext` via
    /// `set_ime_cursor_area`, and the event loop forwards the resulting
    /// `Option<Rect>` here after each render so the IME stays anchored
    /// to the live caret. Without this the candidate window defaults to
    /// a screen-corner position on Windows.
    pub fn set_ime_cursor_area(&self, x: f32, y: f32, width: f32, height: f32) {
        self.window.set_ime_cursor_area(
            LogicalPosition::new(x, y),
            LogicalSize::new(width.max(1.0), height.max(1.0)),
        );
    }

    /// Best-effort snapshot of the OS theme preference for this window.
    ///
    /// Returns `None` when the platform doesn't report a preference
    /// (e.g. X11 outside GNOME / KDE), letting callers fall back to
    /// their compiled-in default. Reactive subscribers should use
    /// `AppScope::system_theme` instead — that signal is also kept
    /// fresh via `WindowEvent::ThemeChanged`.
    pub fn system_theme(&self) -> Option<SystemTheme> {
        self.window.theme().map(SystemTheme::from_winit)
    }
}
