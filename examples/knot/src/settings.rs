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
//! [`signals`], so a change in the UI re-themes every theme-token-driven
//! color without a subtree rebuild and is written back to disk through
//! [`persist`]. A theme swap cross-fades rather than snapping: every
//! paint-time reader goes through [`current_theme`], which eases an
//! [`Animated`] toward the resolved [`target_theme`].
//!
//! Panel colors that don't come from a widget's own theme default (the
//! sidebar / editor backgrounds, selected-row highlight, …) read tokens
//! through the [`current_theme`]-backed helpers in this module
//! ([`surface`], [`on_surface_variant`], …) so they track a theme swap in
//! lockstep with the framework-themed widgets.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use shroud::app::system_theme_signal;
use shroud::core::{Color, Theme};
use shroud::platform::SystemTheme;
use shroud::reactive::{Animated, Easing, Reactive, Signal};
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, TextWidget};

use crate::backup;
use crate::change_password;
use crate::i18n::{self, Key, Language};
use crate::state::AppState;

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

    /// Translation key for this size's settings-modal button label.
    fn key(self) -> Key {
        match self {
            FontSize::Small => Key::SettingsFontSmall,
            FontSize::Medium => Key::SettingsFontMedium,
            FontSize::Large => Key::SettingsFontLarge,
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

    /// Translation key for this choice's settings-modal button label.
    fn key(self) -> Key {
        match self {
            AutoLock::Off => Key::AutoLockOff,
            AutoLock::OneMinute => Key::AutoLockOneMinute,
            AutoLock::FiveMinutes => Key::AutoLockFiveMinutes,
            AutoLock::FifteenMinutes => Key::AutoLockFifteenMinutes,
        }
    }
}

/// How often to automatically back up on unlock. `Off` (the default — backups
/// are opt-in for a privacy app) disables it; the others throttle an unlock-time
/// backup so it runs at most once per period. Discrete + stable on-disk keys for
/// the same reason as the other settings enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoBackup {
    #[default]
    Off,
    Daily,
    Weekly,
}

impl AutoBackup {
    /// Minimum seconds between automatic backups, or `None` when disabled.
    /// `pub(crate)` so the backup module's due-check can read it.
    pub(crate) fn period_secs(self) -> Option<u64> {
        match self {
            AutoBackup::Off => None,
            AutoBackup::Daily => Some(24 * 60 * 60),
            AutoBackup::Weekly => Some(7 * 24 * 60 * 60),
        }
    }

    /// Translation key for this choice's settings-modal button label.
    pub(crate) fn key(self) -> Key {
        match self {
            AutoBackup::Off => Key::AutoBackupOff,
            AutoBackup::Daily => Key::AutoBackupDaily,
            AutoBackup::Weekly => Key::AutoBackupWeekly,
        }
    }
}

/// How the sidebar orders the note list. Pinned notes always float to the
/// top (see `state::compare_notes`); this picks the order *within* the pinned
/// and unpinned groups. `Created` is the historical default (note id order =
/// creation order). Discrete + stable on-disk keys for the same reason as the
/// other settings enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    /// By creation order (ascending note id). The original behavior.
    #[default]
    Created,
    /// By title, case-insensitive, A→Z.
    TitleAsc,
    /// By title, case-insensitive, Z→A.
    TitleDesc,
}

impl SortMode {
    /// Translation key for the sidebar's sort-row button label. Public
    /// because — unlike the other settings enums, whose toggles live in this
    /// module's modal — the sort control is rendered over in `sidebar`.
    pub fn key(self) -> Key {
        match self {
            SortMode::Created => Key::SortCreated,
            SortMode::TitleAsc => Key::SortTitleAsc,
            SortMode::TitleDesc => Key::SortTitleDesc,
        }
    }
}

/// Default backup retention (how many `.knotbak` files to keep). Picked so a
/// few accidental clobbers are recoverable without the backup folder growing
/// without bound.
fn default_retention() -> u32 {
    5
}

/// On-disk settings shape. `#[serde(default)]` on each field means a file
/// written by an older / newer build that's missing a key still loads —
/// the missing field falls back to its `Default`.
///
/// Not `Copy`: `backup_dir` is an owned `String`. Callers clone or move it.
/// `Default` is hand-written (below) rather than derived so `backup_retention`
/// agrees with its `#[serde(default = ...)]` — a derived `Default` would give 0,
/// diverging from the 5 a freshly-deserialized `{}` produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default)]
    pub font_size: FontSize,
    #[serde(default)]
    pub auto_lock: AutoLock,
    #[serde(default)]
    pub sort: SortMode,
    #[serde(default)]
    pub language: Language,
    /// User-chosen backup folder. `None` = use the default under the config
    /// dir (see [`resolved_backup_dir`]). Stored as a string for portable
    /// JSON; resolved to a `PathBuf` on read.
    #[serde(default)]
    pub backup_dir: Option<String>,
    /// How many timestamped `.knotbak` backups to keep before rotating the
    /// oldest out. Clamped to ≥ 1 by the rotation itself.
    #[serde(default = "default_retention")]
    pub backup_retention: u32,
    /// Automatic-backup cadence (on unlock, throttled). Default `Off`.
    #[serde(default)]
    pub auto_backup: AutoBackup,
    /// Unix seconds of the last backup (manual or automatic), `0` = never.
    /// The auto-backup throttle compares this against `now`.
    #[serde(default)]
    pub last_backup_at: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            font_size: FontSize::default(),
            auto_lock: AutoLock::default(),
            sort: SortMode::default(),
            language: Language::default(),
            backup_dir: None,
            backup_retention: default_retention(),
            auto_backup: AutoBackup::default(),
            last_backup_at: 0,
        }
    }
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
    pub sort: Signal<SortMode>,
    pub language: Signal<Language>,
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
                sort: Signal::new(loaded.sort),
                language: Signal::new(loaded.language),
            }
        })
    })
}

/// Resolve the *target* [`Theme`] from the current signal values plus the
/// OS theme — the destination any in-flight theme fade eases toward. A pure
/// function of the settings signals; [`current_theme`] wraps it in the
/// easing layer the rest of the app actually reads.
fn target_theme() -> Theme {
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

/// Duration of the cross-fade between themes. Short enough to feel
/// responsive, long enough to read as a transition rather than a flicker.
const THEME_FADE: Duration = Duration::from_millis(180);

thread_local! {
    /// The currently-*displayed* theme, easing toward [`target_theme`] over
    /// [`THEME_FADE`] whenever the target changes. Thread-local for the same
    /// reason as [`signals`] / `system_theme_signal` — the UI runs
    /// single-threaded on the event-loop thread. Lazily initialized on the
    /// first [`current_theme`] read, resting at the startup target (so the
    /// first frame doesn't fade in from nowhere).
    static DISPLAYED_THEME: OnceCell<Animated<Theme>> = const { OnceCell::new() };
}

/// The theme as currently *displayed*, easing toward [`target_theme`].
///
/// Fed to `App::theme(Reactive::derive(current_theme))` *and* read by every
/// panel-color helper below, so the framework-themed widgets and the app's
/// explicit panel colors cross-fade together on a theme swap (Light ⇄ Dark,
/// or an OS appearance flip under `System`). Font-size changes still apply
/// instantly — [`Theme`]'s `Lerp` snaps typography to the target.
///
/// Called many times per paint; the retarget check is idempotent within a
/// frame (once the animator's target matches, later calls don't restart the
/// fade), and a settled animator casts no frame vote, so the app idles at
/// rest between transitions.
pub fn current_theme() -> Theme {
    let target = target_theme();
    DISPLAYED_THEME.with(|cell| {
        let anim =
            cell.get_or_init(|| Animated::new(target.clone(), THEME_FADE, Easing::EaseInOut));
        if anim.target() != target {
            anim.set(target.clone());
        }
        anim.get()
    })
}

/// Current auto-lock preference. Read by the per-frame tick in `main` to
/// decide whether (and after how long) an idle vault should re-lock.
pub fn current_auto_lock() -> AutoLock {
    signals().auto_lock.get()
}

/// Current sidebar sort order. Read by `sidebar::populate_list` when it asks
/// `AppState::filtered_note_ids` for the ordered note ids.
pub fn current_sort() -> SortMode {
    signals().sort.get()
}

/// Persist the current signal values. Called after a settings change in
/// the modal (or the sidebar's sort row) — the signals are the source of
/// truth for the values they back.
///
/// Backup settings (`backup_dir` / `backup_retention`) are *not* signal-backed
/// — only the backup modal reads them — so we load the existing file and
/// override just the signal-backed fields, preserving the backup keys. (Writing
/// a freshly-constructed `Settings` would reset them to default on every theme
/// change.) The backup modal persists its own fields the same load-merge way.
pub fn persist() {
    let s = signals();
    let mut current = Settings::load();
    current.theme = s.theme.get();
    current.font_size = s.font.get();
    current.auto_lock = s.auto_lock.get();
    current.sort = s.sort.get();
    current.language = s.language.get();
    current.save();
}

// --- Backup settings (not signal-backed; read/written by the backup modal) --

/// The configured backup folder, or the default `<config>/knot/backups/` when
/// the user hasn't chosen one. `None` only if the platform can't report a
/// config dir (persistence unavailable).
pub fn resolved_backup_dir() -> Option<PathBuf> {
    if let Some(dir) = Settings::load().backup_dir {
        return Some(PathBuf::from(dir));
    }
    shroud::platform::storage::config_dir(APP_NAME)
        .ok()
        .map(|d| d.join("backups"))
}

/// The user's explicitly-chosen backup folder, if any (distinct from the
/// resolved default — the modal shows "(default)" when this is `None`).
pub fn configured_backup_dir() -> Option<PathBuf> {
    Settings::load().backup_dir.map(PathBuf::from)
}

/// Persist a chosen backup folder (load-merge so other settings survive).
/// `None` clears the override, reverting to the default location.
pub fn set_backup_dir(dir: Option<PathBuf>) {
    let mut s = Settings::load();
    s.backup_dir = dir.map(|p| p.to_string_lossy().into_owned());
    s.save();
}

/// Current backup-retention count (how many backups to keep).
pub fn backup_retention() -> u32 {
    Settings::load().backup_retention
}

/// Persist a new backup-retention count (load-merge).
pub fn set_backup_retention(keep: u32) {
    let mut s = Settings::load();
    s.backup_retention = keep;
    s.save();
}

/// Current automatic-backup cadence.
pub fn auto_backup() -> AutoBackup {
    Settings::load().auto_backup
}

/// Persist a new automatic-backup cadence (load-merge).
pub fn set_auto_backup(choice: AutoBackup) {
    let mut s = Settings::load();
    s.auto_backup = choice;
    s.save();
}

/// Unix seconds of the last backup (manual or automatic), `0` = never.
pub fn last_backup_at() -> u64 {
    Settings::load().last_backup_at
}

/// Record the time of a backup just taken (load-merge). Read by the
/// auto-backup throttle to decide when the next one is due.
pub fn set_last_backup_at(secs: u64) {
    let mut s = Settings::load();
    s.last_backup_at = secs;
    s.save();
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
/// closure (`FnOnce(&mut WidgetTree, usize)`), so most of it needs no
/// `EventContext` of its own — option clicks only set a signal + persist.
/// The Done button and the change-password row touch `ctx` (to pop, or to
/// push the nested change-password modal); the latter is why `state` is
/// threaded in.
///
/// Each option button highlights when active by deriving its background
/// from the same signal it writes, so the selection reads back live.
pub fn populate_settings_modal(tree: &mut WidgetTree, dialog: usize, state: Rc<RefCell<AppState>>) {
    let s = signals();

    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::SettingsTitle).to_string())
            .font_size(22.0)
            .color(on_surface()),
    );

    // --- Theme ---
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::SettingsTheme).to_string())
            .color(on_surface_variant()),
    );
    let theme_row = tree.add_child(dialog, Container::row().gap(8.0));
    for (label, choice) in [
        (Key::SettingsThemeLight, ThemeChoice::Light),
        (Key::SettingsThemeDark, ThemeChoice::Dark),
        (Key::SettingsThemeSystem, ThemeChoice::System),
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
            Button::reactive_label(move || i18n::tr(label).to_string())
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
        TextWidget::reactive(|| i18n::tr(Key::SettingsFontSize).to_string())
            .color(on_surface_variant()),
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
            Button::reactive_label(move || i18n::tr(size.key()).to_string())
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
        TextWidget::reactive(|| i18n::tr(Key::SettingsAutoLock).to_string())
            .color(on_surface_variant()),
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
            Button::reactive_label(move || i18n::tr(choice.key()).to_string())
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(choice);
                    persist();
                }),
        );
    }

    // --- Language ---
    // Switching this re-renders every reactive string on the next paint
    // (see `crate::i18n`), so the whole UI — including this modal's own
    // labels — flips language live without a tree rebuild.
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::SettingsLanguage).to_string())
            .color(on_surface_variant()),
    );
    let lang_row = tree.add_child(dialog, Container::row().gap(8.0));
    for (label, choice) in [
        (Key::LanguageJa, Language::Ja),
        (Key::LanguageEn, Language::En),
        (Key::LanguageSystem, Language::System),
    ] {
        let sig = s.language;
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
            lang_row,
            Button::reactive_label(move || i18n::tr(label).to_string())
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(choice);
                    persist();
                }),
        );
    }

    // --- Security ---
    // Change the master password. Opens a nested modal that re-wraps the DEK
    // under a new password (the notes are never re-encrypted — see
    // `crate::change_password`). Distinct from the live-applied option rows
    // above: it needs the app state and its own `EventContext` to push the
    // sub-dialog, which is why this builder takes `state`.
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::SettingsSecurity).to_string())
            .color(on_surface_variant()),
    );
    let cp_state = Rc::clone(&state);
    tree.add_child(
        dialog,
        Button::reactive_label(|| i18n::tr(Key::SettingsChangePassword).to_string())
            .radius(6.0)
            .on_click(move |ctx| {
                let st = Rc::clone(&cp_state);
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column()
                        .width(400.0)
                        .padding(24.0)
                        .gap(16.0)
                        .background(surface())
                        .radius(12.0),
                    move |tree, dialog| change_password::populate(tree, dialog, st),
                );
            }),
    );

    // --- Backup & restore ---
    // Save / restore an encrypted snapshot of the whole vault. Like the
    // change-password row it opens a nested modal and needs the app state (the
    // restore path locks the session before overwriting the vault files).
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::SettingsBackup).to_string())
            .color(on_surface_variant()),
    );
    let bk_state = state;
    tree.add_child(
        dialog,
        Button::reactive_label(|| i18n::tr(Key::SettingsBackupBtn).to_string())
            .radius(6.0)
            .on_click(move |ctx| {
                let st = Rc::clone(&bk_state);
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column()
                        .width(440.0)
                        .padding(24.0)
                        .gap(12.0)
                        .background(surface())
                        .radius(12.0),
                    move |tree, dialog| backup::populate(tree, dialog, st),
                );
            }),
    );

    // --- Done ---
    let done_row = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
    tree.add_child(
        done_row,
        Button::reactive_label(|| i18n::tr(Key::SettingsDone).to_string())
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
            sort: SortMode::TitleAsc,
            language: Language::Ja,
            backup_dir: Some("/tmp/knot-backups".to_string()),
            backup_retention: 10,
            auto_backup: AutoBackup::Weekly,
            last_backup_at: 1_700_000_000,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
        // The sort key serializes in its stable snake_case form.
        assert!(json.contains(r#""sort":"title_asc""#));
        // Language serializes in its stable lowercase form.
        assert!(json.contains(r#""language":"ja""#));
    }

    #[test]
    fn missing_fields_fall_back_to_default() {
        // A file from a build that only knew about `theme` must still load —
        // including the older two-field file that predates `auto_lock`.
        let back: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(back.theme, ThemeChoice::Dark);
        assert_eq!(back.font_size, FontSize::Medium);
        assert_eq!(back.auto_lock, AutoLock::FiveMinutes);
        assert_eq!(back.sort, SortMode::Created);
        assert_eq!(back.language, Language::System);
        // Backup keys absent in an older file fall back to their defaults.
        assert_eq!(back.backup_dir, None);
        assert_eq!(back.backup_retention, 5);
        assert_eq!(back.auto_backup, AutoBackup::Off);
        assert_eq!(back.last_backup_at, 0);

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
