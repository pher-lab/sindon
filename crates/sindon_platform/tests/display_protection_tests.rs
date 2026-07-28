use sindon_platform::display_protection::{
    DisplayProtection, DisplayProtectionLevel, DisplayProtectionResult,
};

// ── DisplayProtectionLevel ───────────────────────────────────────

#[test]
fn protection_level_default_is_none() {
    assert_eq!(
        DisplayProtectionLevel::default(),
        DisplayProtectionLevel::None
    );
}

#[test]
fn protection_levels_are_distinct() {
    assert_ne!(
        DisplayProtectionLevel::None,
        DisplayProtectionLevel::ExcludeFromCapture
    );
    assert_ne!(
        DisplayProtectionLevel::ExcludeFromCapture,
        DisplayProtectionLevel::ContentProtection
    );
    assert_ne!(
        DisplayProtectionLevel::None,
        DisplayProtectionLevel::ContentProtection
    );
}

// ── DisplayProtectionResult ──────────────────────────────────────

#[test]
fn result_is_applied() {
    assert!(DisplayProtectionResult::Applied.is_applied());
    assert!(!DisplayProtectionResult::Unsupported.is_applied());
    assert!(!DisplayProtectionResult::NoWindowHandle.is_applied());
    assert!(!DisplayProtectionResult::OsError.is_applied());
}

// ── Platform support queries ─────────────────────────────────────

#[test]
fn platform_supported_returns_consistent_value() {
    // Should not panic and should be deterministic
    let a = DisplayProtection::platform_supported();
    let b = DisplayProtection::platform_supported();
    assert_eq!(a, b);
}

#[test]
fn max_supported_level_is_valid() {
    let level = DisplayProtection::max_supported_level();
    #[cfg(target_os = "windows")]
    assert_eq!(level, DisplayProtectionLevel::ContentProtection);
    // Deliberately one rung lower than Windows, and this assertion is the
    // guard on that. macOS reaches NSWindowSharingNone through winit, which is
    // the ExcludeFromCapture class — weaker even within it, since QuickTime is
    // documented as reading through it. There is no macOS counterpart to
    // WDA_MONITOR, so a future edit that "tidies" this into the Windows arm
    // would invent a DRM-level promise nothing on the platform can keep.
    #[cfg(target_os = "macos")]
    assert_eq!(level, DisplayProtectionLevel::ExcludeFromCapture);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    assert_eq!(level, DisplayProtectionLevel::None);
}

/// The support query and the level table have to agree about macOS, because
/// they are read by different callers: an app branches on
/// `platform_supported()`, while the docs and `App::capture_prevention` quote
/// the level. Before this was wired, both said "nothing here"; the failure
/// mode now is one of them being updated without the other.
#[cfg(target_os = "macos")]
#[test]
fn macos_reports_support_without_claiming_content_protection() {
    assert!(DisplayProtection::platform_supported());
    assert_ne!(
        DisplayProtection::max_supported_level(),
        DisplayProtectionLevel::ContentProtection
    );
}

#[test]
fn platform_supported_matches_max_level() {
    let supported = DisplayProtection::platform_supported();
    let max = DisplayProtection::max_supported_level();

    if supported {
        assert_ne!(max, DisplayProtectionLevel::None);
    } else {
        assert_eq!(max, DisplayProtectionLevel::None);
    }
}
