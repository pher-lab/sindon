use std::sync::Arc;

use crate::display_protection::{DisplayProtection, DisplayProtectionResult};
use crate::system_theme::SystemTheme;
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::dpi::{LogicalSize, PhysicalSize};
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
    /// On Windows we also force-attach a fresh IME context via the IMM32
    /// API — winit 0.30's `set_ime_allowed(true)` calls
    /// `ImmAssociateContextEx(hwnd, NULL, IACE_DEFAULT)` which is meant to
    /// restore the thread's default IME context, but on Windows 11 with
    /// Microsoft IME for Japanese the call silently no-ops (no `Ime::Enabled`
    /// fires, raw Hankaku / Zenkaku keys reach the app as `KeyboardInput`).
    /// Calling `ImmCreateContext` + `ImmAssociateContext` directly attaches
    /// a guaranteed-valid context and is what the user's IME starts
    /// intercepting keystrokes against.
    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
        #[cfg(target_os = "windows")]
        if allowed {
            force_attach_ime_windows(&self.window);
        }
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

/// Best-effort IME bring-up for the given window on Windows.
///
/// Forces the IME open status to `true` via `ImmSetOpenStatus` so a freshly
/// focused window with a Japanese keyboard layout would compose immediately
/// instead of waiting for the user to press 半角/全角.
///
/// **Known limitation (2026-05):** on Win11 + Microsoft IME for Japanese +
/// winit 0.30, even with the IME context attached and `ImmSetOpenStatus`
/// reporting success, the OS IME hook does not intercept `WM_KEYDOWN` for
/// shroud windows. Diagnostic findings (`GetKeyboardLayout` returns
/// Japanese, `ImmGetContext` returns a valid HIMC at every keystroke,
/// `ImmGetOpenStatus` returns `true`, `ImmGetConversionStatus` returns the
/// expected Native+Roman+Fullshape) all line up — yet `Hankaku` / `Zenkaku`
/// / `Convert` named keys reach `KeyboardInput` as raw events, and
/// `WindowEvent::Ime(_)` is never produced. The Win32 API setup looks
/// correct end-to-end; the breakage is somewhere between winit's message
/// dispatch and the IME hook. Tracked as a follow-up phase candidate
/// (custom `msg_hook`, or a winit alternative). Users can paste Japanese
/// text via Ctrl+V in the meantime — the paste path uses the system
/// clipboard and does not go through IME.
///
/// Failures are deliberately silent because IME activation is a UX feature,
/// not a correctness one: a no-op here just means the existing keystroke
/// path (raw `KeyboardInput` → `CharInput` per ASCII char) continues to
/// work, the same as on a system with no IME installed.
#[cfg(target_os = "windows")]
fn force_attach_ime_windows(window: &Window) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::Ime::{ImmGetContext, ImmReleaseContext, ImmSetOpenStatus};

    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(_) => return,
    };

    let hwnd = match handle.as_raw() {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        _ => return,
    };

    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        // Idempotent: if the user has already toggled IME closed (intentional
        // alphanumeric mode for a specific flow) this re-opens it on the
        // next focus event. Acceptable for M1 — apps that need IME-off
        // behavior can call `set_ime_allowed(false)` explicitly per flow.
        let _ = ImmSetOpenStatus(himc, true);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}
