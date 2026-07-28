//! Display protection — prevent screen capture and recording.
//!
//! # Platform support
//!
//! | Platform | Applied? | Why |
//! |----------|----------|-----|
//! | Windows  | yes, both levels | `SetWindowDisplayAffinity` — `WDA_EXCLUDEFROMCAPTURE`, or `WDA_MONITOR` for [`ContentProtection`](DisplayProtectionLevel::ContentProtection) |
//! | macOS    | up to [`ExcludeFromCapture`](DisplayProtectionLevel::ExcludeFromCapture), never observed | `NSWindow.setSharingType(.none)`, through winit |
//! | Linux/X11 | no | No X11 API for capture prevention exists |
//! | Linux/Wayland | no | No standard protocol; a compositor may add an opt-in one later |
//!
//! On Linux every operation returns `false` / `Unsupported`. The column says
//! what the window actually gets, not what the platform could in principle
//! offer: a table that reads "Full" next to code returning `Unsupported` is how
//! a security claim outlives the thing it described.
//!
//! # macOS is not the Windows guarantee
//!
//! Two things separate that row from the one above it, and a
//! [`DisplayProtectionResult`] shows neither.
//!
//! **It protects less.** `NSWindowSharingNone` belongs to the
//! `WDA_EXCLUDEFROMCAPTURE` class, and even inside that class it is the weaker
//! member: winit documents QuickTime as still able to read a window that has
//! it. Nothing on macOS corresponds to `WDA_MONITOR`, so
//! [`ContentProtection`](DisplayProtectionLevel::ContentProtection) reports
//! `Unsupported` there instead of quietly aliasing the level below it.
//!
//! **Nobody has watched it work.** Capture prevention is a claim about what
//! *another* process sees, so the only honest test is a screenshot taken from
//! outside that comes back blank — and a hosted macOS runner grants no screen
//! recording, which is why CI cannot be that test no matter how green it is.
//! Worse, the call it delegates to returns `()`: AppKit's setter reports no
//! failure, so `Applied` on macOS means *the request was made*, where on
//! Windows it means the OS returned success. Until someone takes that
//! screenshot on real hardware, this row is an implementation, not a
//! verification.

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
    /// macOS: `NSWindowSharingNone`, which is weaker — QuickTime is
    /// documented as still able to read such a window.
    ExcludeFromCapture,
    /// Full content protection (DRM-level).
    /// Window content is protected even from hardware capture paths.
    /// Windows: `WDA_MONITOR` — content is replaced with a solid color
    /// in both software and some hardware capture scenarios.
    /// macOS has no equivalent and reports `Unsupported` for this level.
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
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
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
        cfg!(any(target_os = "windows", target_os = "macos"))
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
        // One rung below Windows, on purpose. macOS has no counterpart to
        // WDA_MONITOR, and `NSWindowSharingNone` is weaker than even the level
        // named here (see the module docs). Returning `ContentProtection`
        // would make this function the place a DRM-level promise gets invented.
        #[cfg(target_os = "macos")]
        {
            DisplayProtectionLevel::ExcludeFromCapture
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
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
        // Everything here follows winit 0.30's *implementation*
        // (platform_impl/macos/window_delegate.rs), not its rustdoc, because
        // the two disagree: the public doc says "if `false`,
        // NSWindowSharingNone is used", while the code passes
        // NSWindowSharingNone for `true` and NSWindowSharingReadOnly — the
        // AppKit default — for `false`. Read the source before trusting the
        // sentence; a mapping copied from that doc would invert the feature.
        //
        // `set_content_protected` returns `()`. AppKit's setter has no failure
        // signal at all, so unlike the Windows arm above — which checks what
        // `SetWindowDisplayAffinity` returned — there is nothing here to
        // check. `Applied` therefore means "the request was made", one notch
        // weaker than the same value means on Windows, and no amount of it
        // substitutes for the screenshot-from-outside nobody has taken yet.
        match level {
            DisplayProtectionLevel::None => {
                self.window.set_content_protected(false);
                DisplayProtectionResult::Applied
            }
            DisplayProtectionLevel::ExcludeFromCapture => {
                self.window.set_content_protected(true);
                DisplayProtectionResult::Applied
            }
            // Not folded in with the arm above, which is the whole decision in
            // this function. `ContentProtection` means WDA_MONITOR on Windows:
            // protection that survives capture paths WDA_EXCLUDEFROMCAPTURE
            // does not. macOS has no such tier — NSWindowSharingNone is the
            // weaker class, and winit names QuickTime as reading through it.
            // Aliasing the two would let an app ask for the strongest level,
            // be told `Applied`, and get less than it asked for on one
            // platform only. Refusing is the answer that stays true.
            DisplayProtectionLevel::ContentProtection => DisplayProtectionResult::Unsupported,
        }
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
