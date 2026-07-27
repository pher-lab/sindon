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
    // On Windows, should be ContentProtection
    // On other platforms, should be None
    #[cfg(target_os = "windows")]
    assert_eq!(level, DisplayProtectionLevel::ContentProtection);
    #[cfg(not(target_os = "windows"))]
    assert_eq!(level, DisplayProtectionLevel::None);
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
