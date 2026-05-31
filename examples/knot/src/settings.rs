//! Persistent, non-secret app settings (theme + font size).
//!
//! Stored as plain JSON at `<config>/knot/settings.json`, right next to
//! the vault. These are *not* secret material — they describe app chrome,
//! so they go through the unencrypted Phase 37
//! [`shroud::platform::storage`] helpers rather than the SQLCipher /
//! envelope layers the notes use.
//!
//! The live values are exposed as thread-local [`Signal`]s (mirroring
//! [`shroud::app::system_theme_signal`]). Both the `App::theme(...)`
//! reactive and the settings modal read the *same* handles via
//! [`signals`], so a change in the UI flips every theme-token-driven
//! color on the next paint (no subtree rebuild) and is written back to
//! disk through [`persist`].
//!
//! Panel colors that don't come from a widget's own theme default (the
//! sidebar / editor backgrounds, selected-row highlight, …) read tokens
//! through the [`current_theme`]-backed helpers in this module
//! ([`surface`], [`on_surface_variant`], …) so they track a theme swap in
//! lockstep with the framework-themed widgets.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use shroud::app::system_theme_signal;
use shroud::core::{Color, Theme};
use shroud::platform::SystemTheme;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

const APP_NAME: &str = "knot";
const SETTINGS_FILENAME: &str = "settings.json";

/// Theme preference. `System` follows the OS appearance (the historical
/// default), `Light` / `Dark` pin a specific theme regardless of the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    Light,
    Dark,
    #[default]
    System,
}

/// Discrete font-size step, mapped to a [`Theme::with_font_scale`] factor.
/// Discrete (rather than a free float) keeps the settings UI a simple
/// three-way toggle and the on-disk value stable across versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FontSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl FontSize {
    /// Multiplier handed to [`Theme::with_font_scale`].
    pub fn scale(self) -> f32 {
        match self {
            FontSize::Small => 0.875,
            FontSize::Medium => 1.0,
            FontSize::Large => 1.15,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FontSize::Small => "Small",
            FontSize::Medium => "Medium",
            FontSize::Large => "Large",
        }
    }
}

/// Inactivity timeout before the vault auto-locks. `Off` disables the
/// timer entirely; the others lock the vault (re-encrypt, drop the master
/// key, return to the lock screen) after the given idle span with no user
/// input. Discrete so the settings UI is a simple toggle and the on-disk
/// value stays stable across versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoLock {
    Off,
    #[serde(rename = "1min")]
    OneMinute,
    #[default]
    #[serde(rename = "5min")]
    FiveMinutes,
    #[serde(rename = "15min")]
    FifteenMinutes,
}

impl AutoLock {
    /// Idle duration before locking, or `None` when disabled. The
    /// auto-lock tick compares this against `FrameContext::idle()`.
    pub fn timeout(self) -> Option<Duration> {
        match self {
            AutoLock::Off => None,
            AutoLock::OneMinute => Some(Duration::from_secs(60)),
            AutoLock::FiveMinutes => Some(Duration::from_secs(5 * 60)),
            AutoLock::FifteenMinutes => Some(Duration::from_secs(15 * 60)),
        }
    }

    fn label(self) -> &'static str {
        match self {
            AutoLock::Off => "Off",
            AutoLock::OneMinute => "1 min",
            AutoLock::FiveMinutes => "5 min",
            AutoLock::FifteenMinutes => "15 min",
        }
    }
}

/// On-disk settings shape. `#[serde(default)]` on each field means a file
/// written by an older / newer build that's missing a key still loads —
/// the missing field falls back to its `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default)]
    pub font_size: FontSize,
    #[serde(default)]
    pub auto_lock: AutoLock,
}

fn settings_path() -> Option<PathBuf> {
    let dir = shroud::platform::storage::config_dir(APP_NAME).ok()?;
    Some(dir.join(SETTINGS_FILENAME))
}

impl Settings {
    /// Load from disk, falling back to defaults on a missing file or any
    /// read / parse error. Settings are non-critical chrome — a corrupt
    /// file must not block launch, so we log and reset to defaults rather
    /// than propagate (unlike the vault, where a read failure is fatal).
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        match shroud::platform::storage::read_json::<Settings>(&path) {
            Ok(Some(s)) => s,
            Ok(None) => Self::default(),
            Err(e) => {
                eprintln!("knot: failed to read settings ({e}); using defaults");
                Self::default()
            }
        }
    }

    /// Atomically write to disk. Failures are logged, not propagated —
    /// losing a settings write is a cosmetic regret, not a data-loss
    /// event, and surfacing it would need UI that isn't worth it here.
    pub fn save(&self) {
        let Some(path) = settings_path() else {
            eprintln!("knot: config dir unavailable; settings not saved");
            return;
        };
        if let Err(e) = shroud::platform::storage::write_json_atomic(&path, self) {
            eprintln!("knot: failed to save settings: {e}");
        }
    }
}

/// Live handles to the two settings values. `Copy` (both fields are
/// `Signal`, which is a cheap `Copy` id) so it can be captured into as
/// many closures as needed.
#[derive(Clone, Copy)]
pub struct SettingsSignals {
    pub theme: Signal<ThemeChoice>,
    pub font: Signal<FontSize>,
    pub auto_lock: Signal<AutoLock>,
}

thread_local! {
    static SIGNALS: OnceCell<SettingsSignals> = const { OnceCell::new() };
}

/// Thread-local settings signals, lazily initialized from disk on first
/// access. Mirrors [`shroud::app::system_theme_signal`]: both the
/// `App::theme` reactive and the settings modal call this and get the
/// same handles, so they stay in sync without being threaded through the
/// screen builders.
pub fn signals() -> SettingsSignals {
    SIGNALS.with(|c| {
        *c.get_or_init(|| {
            let loaded = Settings::load();
            SettingsSignals {
                theme: Signal::new(loaded.theme),
                font: Signal::new(loaded.font_size),
                auto_lock: Signal::new(loaded.auto_lock),
            }
        })
    })
}

/// Resolve the active [`Theme`] from the current signal values plus the
/// OS theme. Fed to `App::theme(Reactive::derive(current_theme))` *and*
/// read by every panel-color helper below, so the framework-themed
/// widgets and the app's explicit panel colors flip together.
pub fn current_theme() -> Theme {
    let s = signals();
    let base = match s.theme.get() {
        ThemeChoice::Light => Theme::light(),
        ThemeChoice::Dark => Theme::dark(),
        // `None` = OS appearance not reported (Linux outside GNOME/KDE);
        // fall back to dark, shroud's historical default.
        ThemeChoice::System => match system_theme_signal().get() {
            Some(SystemTheme::Light) => Theme::light(),
            Some(SystemTheme::Dark) | None => Theme::dark(),
        },
    };
    base.with_font_scale(s.font.get().scale())
}

/// Current auto-lock preference. Read by the per-frame tick in `main` to
/// decide whether (and after how long) an idle vault should re-lock.
pub fn current_auto_lock() -> AutoLock {
    signals().auto_lock.get()
}

/// Persist the current signal values. Called after a settings change in
/// the modal — the signals are the source of truth, so we just snapshot
/// them and write.
pub fn persist() {
    let s = signals();
    Settings {
        theme: s.theme.get(),
        font_size: s.font.get(),
        auto_lock: s.auto_lock.get(),
    }
    .save();
}

// --- Reactive theme-token color helpers ------------------------------------
//
// Each returns a `Reactive<Color>` that re-reads `current_theme()` on every
// paint, so a panel painted with `settings::surface()` tracks a theme swap.
// Call sites read cleanly: `.background(settings::surface())`.

/// App canvas / main content background (`theme.colors.background`).
pub fn background() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.background)
}

/// Raised panel surface — sidebar, cards (`theme.colors.surface`).
pub fn surface() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.surface)
}

/// Secondary / muted text (`theme.colors.on_surface_variant`).
pub fn on_surface_variant() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.on_surface_variant)
}

/// Primary-emphasis text on a surface (`theme.colors.on_surface`).
pub fn on_surface() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.on_surface)
}

/// Hover surface for interactive rows (`theme.hover.bg`).
pub fn hover() -> Reactive<Color> {
    Reactive::derive(|| current_theme().hover.bg)
}

/// Error / warning text (`theme.colors.error` / `.warning`).
pub fn error() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.error)
}

pub fn warning() -> Reactive<Color> {
    Reactive::derive(|| current_theme().colors.warning)
}

// --- Settings modal --------------------------------------------------------

/// Populate the settings dialog body. Used as the `push_layer` populate
/// closure (`FnOnce(&mut WidgetTree, usize)`), so it needs no
/// `EventContext` of its own — option clicks only set a signal + persist,
/// and only the Done button touches `ctx` (to pop the layer).
///
/// Each option button highlights when active by deriving its background
/// from the same signal it writes, so the selection reads back live.
pub fn populate_settings_modal(tree: &mut WidgetTree, dialog: usize) {
    let s = signals();

    tree.add_child(
        dialog,
        TextWidget::new("Settings")
            .font_size(22.0)
            .color(on_surface()),
    );

    // --- Theme ---
    tree.add_child(dialog, TextWidget::new("Theme").color(on_surface_variant()));
    let theme_row = tree.add_child(dialog, Container::row().gap(8.0));
    for (label, choice) in [
        ("Light", ThemeChoice::Light),
        ("Dark", ThemeChoice::Dark),
        ("System", ThemeChoice::System),
    ] {
        let sig = s.theme;
        let bg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == choice {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == choice {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        tree.add_child(
            theme_row,
            Button::new(label)
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(choice);
                    persist();
                }),
        );
    }

    // --- Font size ---
    tree.add_child(
        dialog,
        TextWidget::new("Font size").color(on_surface_variant()),
    );
    let font_row = tree.add_child(dialog, Container::row().gap(8.0));
    for size in [FontSize::Small, FontSize::Medium, FontSize::Large] {
        let sig = s.font;
        let bg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == size {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == size {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        tree.add_child(
            font_row,
            Button::new(size.label())
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(size);
                    persist();
                }),
        );
    }

    // --- Auto-lock ---
    tree.add_child(
        dialog,
        TextWidget::new("Auto-lock").color(on_surface_variant()),
    );
    let lock_row = tree.add_child(dialog, Container::row().gap(8.0));
    for choice in [
        AutoLock::Off,
        AutoLock::OneMinute,
        AutoLock::FiveMinutes,
        AutoLock::FifteenMinutes,
    ] {
        let sig = s.auto_lock;
        let bg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == choice {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg = Reactive::derive(move || {
            let t = current_theme();
            if sig.get() == choice {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        tree.add_child(
            lock_row,
            Button::new(choice.label())
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(choice);
                    persist();
                }),
        );
    }

    // --- Done ---
    let done_row = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
    tree.add_child(
        done_row,
        Button::new("Done")
            .radius(6.0)
            .on_click(|ctx| ctx.pop_top_layer()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_system_and_medium() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemeChoice::System);
        assert_eq!(s.font_size, FontSize::Medium);
    }

    #[test]
    fn settings_json_round_trips() {
        let original = Settings {
            theme: ThemeChoice::Light,
            font_size: FontSize::Large,
            auto_lock: AutoLock::FifteenMinutes,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn missing_fields_fall_back_to_default() {
        // A file from a build that only knew about `theme` must still load —
        // including the older two-field file that predates `auto_lock`.
        let back: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(back.theme, ThemeChoice::Dark);
        assert_eq!(back.font_size, FontSize::Medium);
        assert_eq!(back.auto_lock, AutoLock::FiveMinutes);

        // And an empty object loads to all-defaults.
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Settings::default());
    }

    #[test]
    fn font_scale_is_monotonic() {
        assert!(FontSize::Small.scale() < FontSize::Medium.scale());
        assert!(FontSize::Medium.scale() < FontSize::Large.scale());
    }

    #[test]
    fn auto_lock_default_is_five_minutes() {
        assert_eq!(AutoLock::default(), AutoLock::FiveMinutes);
        assert_eq!(
            AutoLock::FiveMinutes.timeout(),
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn auto_lock_off_has_no_timeout() {
        assert_eq!(AutoLock::Off.timeout(), None);
    }

    #[test]
    fn auto_lock_serializes_with_stable_keys() {
        // The on-disk keys must stay stable across versions — assert the
        // exact rename strings so a careless edit can't silently break
        // existing settings files.
        assert_eq!(
            serde_json::to_string(&AutoLock::OneMinute).unwrap(),
            r#""1min""#
        );
        let back: AutoLock = serde_json::from_str(r#""15min""#).unwrap();
        assert_eq!(back, AutoLock::FifteenMinutes);
    }
}
