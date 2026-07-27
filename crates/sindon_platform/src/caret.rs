//! OS caret-blink preference.
//!
//! Returns the system's caret blink half-period, or `None` when the user has
//! turned blinking off entirely — an accessibility choice a UI framework should
//! honour rather than override. `sindon_app` reads this once at startup and
//! publishes it to the widget layer; widgets never call it directly.
//!
//! Like `system_locale` this is a one-shot snapshot.
//! The preference can change while the process runs (the Control Panel keyboard
//! slider), but that's rare and isn't surfaced as a winit event, so live
//! updates are out of scope — re-query at app start if a fresher value matters.

use std::time::Duration;

/// The Windows default blink half-period, used as the fallback when the OS
/// can't be queried and on platforms without a blink-rate API.
const DEFAULT_BLINK: Duration = Duration::from_millis(530);

/// Best-effort snapshot of the OS caret-blink half-period.
///
/// `Some(interval)` is the time the caret spends solid before toggling (equal
/// to the time it then spends hidden). `None` means the user disabled blinking
/// — the caret should stay solid.
///
/// On Windows this reads `GetCaretBlinkTime`, whose `INFINITE` return is the
/// "don't blink" signal. Elsewhere there is no portable per-user blink setting,
/// so it returns the platform-conventional default.
#[cfg(windows)]
pub fn caret_blink_time() -> Option<Duration> {
    use windows::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime;

    // SAFETY: `GetCaretBlinkTime` takes no arguments and only reads a
    // process-global system metric; there are no invariants to uphold.
    let ms = unsafe { GetCaretBlinkTime() };
    match ms {
        // 0 is the documented error return — fall back to the default rate.
        0 => Some(DEFAULT_BLINK),
        // INFINITE (0xFFFFFFFF): the user turned caret blinking off.
        u32::MAX => None,
        ms => Some(Duration::from_millis(u64::from(ms))),
    }
}

/// Non-Windows fallback: no portable per-user blink rate, so use the default.
#[cfg(not(windows))]
pub fn caret_blink_time() -> Option<Duration> {
    Some(DEFAULT_BLINK)
}
