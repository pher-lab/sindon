//! Integration tests for `SystemTheme`.
//!
//! The reactive end-to-end (signal updates on `ThemeChanged`) is
//! covered in `sindon_app` because it needs a live event loop. Here
//! we only verify the public conversion surface — the part that
//! actually has logic worth pinning down.

use sindon_platform::SystemTheme;
use winit::window::Theme as WinitTheme;

#[test]
fn from_winit_maps_light_to_light() {
    assert_eq!(
        SystemTheme::from_winit(WinitTheme::Light),
        SystemTheme::Light
    );
}

#[test]
fn from_winit_maps_dark_to_dark() {
    assert_eq!(SystemTheme::from_winit(WinitTheme::Dark), SystemTheme::Dark);
}

#[test]
fn enum_derives_match_expected() {
    // SystemTheme being Copy + Eq + Hash is part of the public
    // contract — Signal<Option<SystemTheme>>::get() relies on Copy,
    // and apps often switch over it. Catch accidental derive removals
    // with a compile-time exercise of each trait.
    let t = SystemTheme::Dark;
    let _copy = t;
    let _eq = t == SystemTheme::Dark;
    let mut set = std::collections::HashSet::new();
    set.insert(t);
    assert!(set.contains(&SystemTheme::Dark));
}
