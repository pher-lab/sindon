//! Smoke tests for `system_locale`.
//!
//! Real values are environment-dependent (the CI runner's `LANG`
//! could be anything) so these only assert that the call doesn't
//! panic and, *if* it returns a value, that value is shaped like a
//! locale tag rather than an empty string or stray whitespace. The
//! actual sys-locale crate carries its own per-platform tests for the
//! lookup logic — duplicating those here would only re-test
//! upstream.

use shroud_platform::system_locale;

#[test]
fn system_locale_does_not_panic() {
    let _ = system_locale();
}

#[test]
fn system_locale_returns_non_empty_when_present() {
    if let Some(locale) = system_locale() {
        assert!(
            !locale.trim().is_empty(),
            "system_locale returned a Some that's empty/whitespace: {locale:?}"
        );
        // A BCP-47 tag must start with a letter (language subtag) —
        // sys-locale already normalizes, but pin the shape so a future
        // wrapper change can't silently start returning, say, the raw
        // LANG value with a leading `.`.
        let first = locale.chars().next().unwrap();
        assert!(
            first.is_ascii_alphabetic(),
            "expected locale tag to start with a letter, got {locale:?}"
        );
    }
}
