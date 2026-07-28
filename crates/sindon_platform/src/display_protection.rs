//! Display protection — prevent screen capture and recording.
//!
//! # Platform support
//!
//! | Platform | Applied? | Why |
//! |----------|----------|-----|
//! | Windows  | yes | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` |
//! | macOS    | no  | Reachable — winit's `set_content_protected` calls `NSWindow.setSharingType(.none)` — but not wired up, because nothing can observe the result without real hardware |
//! | Linux/X11 | no | No X11 API for capture prevention exists |
//! | Linux/Wayland | no | No standard protocol; a compositor may add an opt-in one later |
//!
//! Everywhere except Windows, all operations return `false` / `Unsupported`.
//! The column says what the window actually gets, not what the platform could
//! in principle offer: a table that reads "Full" next to code returning
//! `Unsupported` is how a security claim outlives the thing it described.

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use winit::window::Window;

/// Protection level for the window's display content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DisplayProtectionLevel {
    /// No protection — window content is visible to screen capture.
    #[default]
    None,
    /// Exclude from screen capture and recording.
    /// Window appears as a black rectangle in screenshots/recordings.
    /// Windows: `WDA_EXCLUDEFROMCAPTURE` (Win10 2004+).
    ExcludeFromCapture,
    /// Full content protection (DRM-level).
    /// Window content is protected even from hardware capture paths.
    /// Windows: `WDA_MONITOR` — content is replaced with a solid color
    /// in both software and some hardware capture scenarios.
    ContentProtection,
}

/// Result of a display protection operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayProtectionResult {
    /// Protection was applied successfully.
    Applied,
    /// The requested level is not supported on this platform/version.
    Unsupported,
    /// Failed to obtain the window handle.
    NoWindowHandle,
    /// The OS API call failed.
    OsError,
}

impl DisplayProtectionResult {
    /// Returns `true` if protection was successfully applied.
    pub fn is_applied(self) -> bool {
        self == Self::Applied
    }
}

/// Query and apply display protection for a window.
pub struct DisplayProtection {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    window: Arc<Window>,
    current_level: DisplayProtectionLevel,
}

impl DisplayProtection {
    /// Create a new display protection manager for the given window.
    pub fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            current_level: DisplayProtectionLevel::None,
        }
    }

    /// Get the current protection level.
    pub fn current_level(&self) -> DisplayProtectionLevel {
        self.current_level
    }

    /// Check if any display protection is supported on this platform.
    pub fn is_supported(&self) -> bool {
        Self::platform_supported()
    }

    /// Static check for platform support (no window needed).
    pub fn platform_supported() -> bool {
        cfg!(target_os = "windows")
        // macOS: reachable through winit but deliberately not wired — see
        // apply_level_macos for why, and what would change it
        // Linux: false (no universal API)
    }

    /// Query the maximum protection level supported on this platform.
    pub fn max_supported_level() -> DisplayProtectionLevel {
        #[cfg(target_os = "windows")]
        {
            // WDA_EXCLUDEFROMCAPTURE requires Win10 2004+ (build 19041)
            // WDA_MONITOR has been available since Win7
            DisplayProtectionLevel::ContentProtection
        }
        #[cfg(not(target_os = "windows"))]
        {
            DisplayProtectionLevel::None
        }
    }

    /// Set the display protection level.
    ///
    /// Returns whether the operation succeeded.
    pub fn set_level(&mut self, level: DisplayProtectionLevel) -> DisplayProtectionResult {
        let result = self.apply_level(level);
        if result.is_applied() {
            self.current_level = level;
        }
        result
    }

    /// Enable capture prevention (convenience for `set_level(ExcludeFromCapture)`).
    pub fn enable(&mut self) -> DisplayProtectionResult {
        self.set_level(DisplayProtectionLevel::ExcludeFromCapture)
    }

    /// Disable all protection (convenience for `set_level(None)`).
    pub fn disable(&mut self) -> DisplayProtectionResult {
        self.set_level(DisplayProtectionLevel::None)
    }

    fn apply_level(&self, level: DisplayProtectionLevel) -> DisplayProtectionResult {
        #[cfg(target_os = "windows")]
        {
            self.apply_level_windows(level)
        }
        #[cfg(target_os = "macos")]
        {
            self.apply_level_macos(level)
        }
        #[cfg(target_os = "linux")]
        {
            self.apply_level_linux(level)
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = level;
            DisplayProtectionResult::Unsupported
        }
    }

    // ── Windows ──────────────────────────────────────────────────

    #[cfg(target_os = "windows")]
    fn apply_level_windows(&self, level: DisplayProtectionLevel) -> DisplayProtectionResult {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE,
            WINDOW_DISPLAY_AFFINITY,
        };

        let handle = match self.window.window_handle() {
            Ok(h) => h,
            Err(_) => return DisplayProtectionResult::NoWindowHandle,
        };

        let hwnd = match handle.as_raw() {
            RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
            _ => return DisplayProtectionResult::NoWindowHandle,
        };

        let affinity: WINDOW_DISPLAY_AFFINITY = match level {
            DisplayProtectionLevel::None => WDA_NONE,
            DisplayProtectionLevel::ExcludeFromCapture => WDA_EXCLUDEFROMCAPTURE,
            DisplayProtectionLevel::ContentProtection => WDA_MONITOR,
        };

        let ok = unsafe { SetWindowDisplayAffinity(hwnd, affinity).is_ok() };
        if ok {
            DisplayProtectionResult::Applied
        } else {
            DisplayProtectionResult::OsError
        }
    }

    // ── macOS ────────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    fn apply_level_macos(&self, level: DisplayProtectionLevel) -> DisplayProtectionResult {
        // Unsupported by choice, not for want of an API — and the reason is
        // not the one this comment used to give. It said a full implementation
        // needed objc2 integration to reach `NSWindow.setSharingType(.none)`.
        // That has not been true for as long as we have depended on winit:
        // `Window::set_content_protected(true)` calls exactly that, and winit
        // already routes it per platform (Windows gets WDA_EXCLUDEFROMCAPTURE,
        // the same affinity the Windows arm above sets by hand).
        //
        // What is actually missing is a way to verify it. Capture prevention
        // is a claim about what another process sees, and the only honest test
        // is to take a screenshot from outside and find it blank. macOS CI
        // cannot do that -- a hosted runner grants no screen recording -- so
        // shipping this would mean turning a documented no-op into an
        // undocumented "probably", on the one platform where nobody has
        // watched sindon draw a single frame. A framework that names secrets
        // in its pitch does not get to guess about that.
        //
        // Trigger: real Mac hardware. It is a one-line call plus a mapping
        // decision (winit's bool cannot distinguish ExcludeFromCapture from
        // ContentProtection, which on Windows are different affinities).
        let _ = level;
        DisplayProtectionResult::Unsupported
    }

    // ── Linux ────────────────────────────────────────────────────

    #[cfg(target_os = "linux")]
    fn apply_level_linux(&self, level: DisplayProtectionLevel) -> DisplayProtectionResult {
        // Linux has no universal screen capture prevention API.
        //
        // X11: No protocol extension for capture prevention exists.
        //      The compositor has full access to all window pixmaps.
        //
        // Wayland: The security model is better (clients can't read
        //          other clients' buffers), but there is no standard
        //          protocol for a client to request "don't screenshot me."
        //          Some compositors (KDE Plasma 6+) may add opt-in APIs
        //          in the future.
        //
        // Mitigation: On Linux, the framework relies on:
        //   1. Secure atlas (GPU glyph data cleared per-frame)
        //   2. Memory protection (mlock, core dump prevention)
        //   3. Process protection (ptrace prevention)
        //
        // The window contents can still be captured, but the sensitive
        // data lifetime in memory is minimized.
        let _ = level;
        DisplayProtectionResult::Unsupported
    }
}

impl std::fmt::Debug for DisplayProtection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayProtection")
            .field("current_level", &self.current_level)
            .field("supported", &self.is_supported())
            .finish()
    }
}
