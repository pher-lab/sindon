//! Design tokens for the Knot UI reproduction.
//!
//! The reference app (`knot-notes-app-v0.7.0`) is React + Tailwind, so its
//! visual language *is* the Tailwind palette with class-based dark mode. We
//! encode that palette here and assemble it into shroud [`Theme`]s (light +
//! dark), exposing the active theme + a dark-mode toggle as reactive helpers.
//!
//! Faithfully expressing Knot's tokens through [`Theme`] is itself a gap
//! probe — anything the model can't carry gets logged in
//! `docs/knot-ui-repro-gaps.md`.
//!
//! This is the *complete* Tailwind palette the reference app draws from, kept
//! whole on purpose: each screen reproduced pulls in more of it. Shades not yet
//! referenced are expected, so the module opts out of dead-code warnings rather
//! than churn `#[allow]`s shade-by-shade as screens land.
#![allow(dead_code)]

use std::cell::OnceCell;

use shroud::core::{Color, Theme};
use shroud::reactive::{Reactive, Signal};

// --- Tailwind palette ------------------------------------------------------
// Standard Tailwind hex values, the literal source-of-truth for the reference
// app's `bg-gray-100` / `text-blue-600` / … utility classes.

// Tailwind hex → shroud `Color`. `from_rgba8` divides bytes by 255 and the
// pipeline displays them as-is on the real screen (the earlier "washed out"
// reading was an HDR→SDR *screenshot* artifact, not the app — see G8 note in
// docs/knot-ui-repro-gaps.md). So no sRGB decode here.
#[inline]
fn c(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

pub fn gray_50() -> Color {
    c(0xf9, 0xfa, 0xfb)
}
pub fn gray_100() -> Color {
    c(0xf3, 0xf4, 0xf6)
}
pub fn gray_200() -> Color {
    c(0xe5, 0xe7, 0xeb)
}
pub fn gray_300() -> Color {
    c(0xd1, 0xd5, 0xdb)
}
pub fn gray_400() -> Color {
    c(0x9c, 0xa3, 0xaf)
}
pub fn gray_500() -> Color {
    c(0x6b, 0x72, 0x80)
}
pub fn gray_600() -> Color {
    c(0x4b, 0x55, 0x63)
}
pub fn gray_700() -> Color {
    c(0x37, 0x41, 0x51)
}
pub fn gray_800() -> Color {
    c(0x1f, 0x29, 0x37)
}
pub fn gray_900() -> Color {
    c(0x11, 0x18, 0x27)
}
pub fn white() -> Color {
    Color::WHITE
}
pub fn blue_400() -> Color {
    c(0x60, 0xa5, 0xfa)
}
pub fn blue_500() -> Color {
    c(0x3b, 0x82, 0xf6)
}
pub fn blue_600() -> Color {
    c(0x25, 0x63, 0xeb)
}
pub fn blue_700() -> Color {
    c(0x1d, 0x4e, 0xd8)
}
pub fn blue_800() -> Color {
    c(0x1e, 0x40, 0xaf)
}
pub fn red_100() -> Color {
    c(0xfe, 0xe2, 0xe2)
}
pub fn red_200() -> Color {
    c(0xfe, 0xca, 0xca)
}
pub fn red_300() -> Color {
    c(0xfc, 0xa5, 0xa5)
}
pub fn red_500() -> Color {
    c(0xef, 0x44, 0x44)
}
pub fn red_700() -> Color {
    c(0xb9, 0x1c, 0x1c)
}
pub fn red_900() -> Color {
    c(0x7f, 0x1d, 0x1d)
}

// --- Theme assembly --------------------------------------------------------
// Map Tailwind tokens onto shroud's `Colors`. Anything the `Theme` model
// can't carry (e.g. a distinct *border* token for inputs vs. panels) is a gap.

fn light_theme() -> Theme {
    let mut t = Theme::light();
    let cols = &mut t.colors;
    cols.background = gray_100();
    cols.surface = white();
    cols.surface_variant = gray_200();
    cols.on_background = gray_900();
    cols.on_surface = gray_900();
    cols.on_surface_variant = gray_500();
    cols.primary = blue_600();
    cols.on_primary = white();
    cols.primary_hover = blue_700();
    cols.primary_pressed = blue_800();
    cols.input_background = white();
    cols.input_background_focused = white();
    cols.input_border = gray_300();
    cols.input_border_focused = blue_500();
    cols.input_placeholder = gray_400();
    cols.error = red_700();
    t
}

fn dark_theme() -> Theme {
    let mut t = Theme::dark();
    let cols = &mut t.colors;
    cols.background = gray_900();
    cols.surface = gray_800();
    cols.surface_variant = gray_700();
    cols.on_background = gray_100();
    cols.on_surface = gray_100();
    cols.on_surface_variant = gray_400();
    cols.primary = blue_600();
    cols.on_primary = white();
    cols.primary_hover = blue_700();
    cols.primary_pressed = blue_800();
    cols.input_background = gray_800();
    cols.input_background_focused = gray_800();
    cols.input_border = gray_700();
    cols.input_border_focused = blue_500();
    cols.input_placeholder = gray_500();
    cols.error = red_500();
    t
}

// --- Live dark-mode toggle -------------------------------------------------
// Thread-local signal (single-threaded UI), default light to match the spec
// we derived from the CSS first. Ctrl+D flips it so we can capture both
// palettes from one binary.

thread_local! {
    static DARK: OnceCell<Signal<bool>> = const { OnceCell::new() };
}

pub fn dark_signal() -> Signal<bool> {
    DARK.with(|cell| *cell.get_or_init(|| Signal::new(false)))
}

pub fn is_dark() -> bool {
    dark_signal().get()
}

pub fn toggle_dark() {
    let s = dark_signal();
    s.set(!s.get());
}

/// The active theme, re-derived per paint. Fed to `App::theme(...)`.
pub fn theme() -> Theme {
    if is_dark() {
        dark_theme()
    } else {
        light_theme()
    }
}

// --- Reactive color helpers ------------------------------------------------
// Each re-reads the active theme on every paint, so panels track the toggle.

pub fn background() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.background)
}
pub fn surface() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.surface)
}
pub fn on_surface() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.on_surface)
}
pub fn muted() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.on_surface_variant)
}
pub fn primary() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.primary)
}
pub fn primary_hover() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.primary_hover)
}
pub fn error() -> Reactive<Color> {
    Reactive::derive(|| theme().colors.error)
}

/// Pick a fixed pair of colors by current mode — for the many Tailwind
/// `bg-x dark:bg-y` cases that don't map to a single semantic token.
pub fn pick(light: Color, dark: Color) -> Reactive<Color> {
    Reactive::derive(move || if is_dark() { dark } else { light })
}
