//! OS-provided locale detection (BCP-47 tag).
//!
//! Wraps [`sys_locale::get_locale`] so callers don't reach past the
//! sindon facade. Used by apps that expose a `language = "system"`
//! preference (Knot's `languageStore.getSystemLanguage` is the
//! motivating case): the returned tag is a hint, not a guarantee —
//! apps should still fall back to a known-supported language when the
//! reported tag isn't one they ship translations for.
//!
//! The lookup is a one-shot snapshot at call time. Locale changes
//! during the process lifetime are uncommon on every supported
//! platform and aren't surfaced as winit events, so live updates are
//! deliberately out of scope. Re-query at app start (or when the user
//! reopens a Settings screen) if a fresher value is wanted.

/// Best-effort snapshot of the OS locale, as a BCP-47 tag
/// (e.g. `"ja-JP"`, `"en-US"`).
///
/// Returns `None` when the OS could not be queried — extremely rare on
/// Windows/macOS, slightly more common on Linux where it depends on
/// the `LANG` / `LC_*` environment.
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}
