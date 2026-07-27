//! Vault backup + restore (feature-parity Tier 2 #6, core).
//!
//! A Knot vault is **four files** that only make sense together:
//!
//! ```text
//! vault.db       — SQLCipher database, page-encrypted under the DEK
//! vault.salt     — Argon2 salt for the password KDF
//! dek.enc        — DEK wrapped under the password-derived KEK
//! recovery.enc   — DEK wrapped under the BIP39 recovery KEK (optional)
//! ```
//!
//! Restoring just `vault.db` is useless — without the matching salt + wrapped
//! DEK no password can unlock it. So a backup is a *snapshot of the whole set*,
//! packed into one portable `.knotbak` file (one file is easy to move offsite;
//! everything in it is already encrypted, so it's safe at rest anywhere).
//!
//! ## Container format (self-built, no archive dependency)
//!
//! ```text
//! magic     : 8 bytes  = b"KNOTBAK1"   (last byte = format version)
//! count     : u32 LE   = number of entries
//! entry × N :
//!   name_len: u32 LE
//!   name    : UTF-8 bytes  (the vault file name, e.g. "vault.db")
//!   data_len: u64 LE
//!   data    : raw file bytes
//! ```
//!
//! Restore only ever writes back to the four known [`VaultPaths`] slots, keyed
//! by file name — an unknown name in the container is ignored, never written,
//! so a tampered archive can't drop a file outside the vault directory.
//!
//! ## Restore is destructive — the caller must lock first
//!
//! Restore overwrites the live vault files. Two ordering rules the caller
//! ([`crate::settings`]'s backup modal) must follow, both for correctness:
//!
//! 1. **Close the open DB connection first.** While unlocked, `VaultStorage`
//!    holds `vault.db` open; on Windows that blocks overwriting it. The caller
//!    drops the `Unlocked` phase (`AppState::discard_and_lock`) before
//!    [`commit_restore`].
//! 2. **Don't flush on the way out.** A normal lock re-encrypts the in-memory
//!    notes into the DB — which would clobber the file we just restored. So the
//!    caller uses `discard_and_lock` (no flush), not `lock_and_seal`.
//!
//! [`prepare_restore`] validates the file without writing (so a bad pick can't
//! lock the user out) and yields the restored salt, which the caller mirrors
//! into `AppState::salt` (the unlock path reads the cached salt, not the disk).

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use sindon::platform::FileDialog;
use sindon::reactive::{Reactive, Signal};
use sindon::widgets::layer::LayerOptions;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, EventContext, Input, TextWidget};

use crate::crypto::SALT_SIZE;
use crate::i18n::{self, Key};
use crate::lock_screen;
use crate::notice;
use crate::settings::{self, AutoBackup};
use crate::state::AppState;
use crate::storage::VaultPaths;

/// Container magic + version. Bump the trailing byte if the format changes.
const MAGIC: &[u8; 8] = b"KNOTBAK1";

/// File extension for a packed backup.
pub const BACKUP_EXT: &str = "knotbak";

/// Upper bound on a backup we'll read back in, a guard against a corrupt or
/// hostile header claiming a huge length. A real vault (DB + a few wrapped-key
/// blobs) is comfortably under this even with a lot of embedded images.
const MAX_BACKUP_LEN: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub enum BackupError {
    Io(io::Error),
    /// The container is malformed: bad magic, truncated, oversized, or missing
    /// a file a vault can't do without.
    Format(String),
}

impl From<io::Error> for BackupError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Format(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for BackupError {}

/// The vault files that make up a backup, in pack order. `recovery.enc` is
/// optional (a vault is usable without it), the rest are required.
fn entries(paths: &VaultPaths) -> [(&Path, bool); 4] {
    [
        (paths.db.as_path(), true),
        (paths.salt.as_path(), true),
        (paths.dek.as_path(), true),
        (paths.recovery.as_path(), false),
    ]
}

/// The file name of `path` as a `String`, or a `Format` error if it has none
/// (shouldn't happen for the resolved vault paths).
fn file_name_of(path: &Path) -> Result<String, BackupError> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BackupError::Format(format!("vault path has no file name: {path:?}")))
}

/// Read the four vault files and serialize them into the `.knotbak` container.
/// A missing *optional* file (`recovery.enc`) is skipped; a missing *required*
/// file surfaces as the underlying `Io` error.
fn pack(paths: &VaultPaths) -> Result<Vec<u8>, BackupError> {
    let mut present: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, required) in entries(paths) {
        match std::fs::read(path) {
            Ok(data) => present.push((file_name_of(path)?, data)),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !required => {}
            Err(e) => return Err(e.into()),
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(present.len() as u32).to_le_bytes());
    for (name, data) in &present {
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Parse a `.knotbak` container into its `(name, data)` entries. Validates the
/// magic and every length against the buffer so a truncated or hostile file
/// fails cleanly instead of panicking on an out-of-range slice.
fn unpack(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, BackupError> {
    let mut pos = 0usize;
    let take = |pos: &mut usize, n: usize| -> Result<&[u8], BackupError> {
        let end = pos
            .checked_add(n)
            .filter(|e| *e <= bytes.len())
            .ok_or_else(|| BackupError::Format("backup file is truncated".into()))?;
        let slice = &bytes[*pos..end];
        *pos = end;
        Ok(slice)
    };

    if take(&mut pos, MAGIC.len())? != MAGIC {
        return Err(BackupError::Format("not a Knot backup (bad header)".into()));
    }
    let count = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap());

    let mut out = Vec::new();
    for _ in 0..count {
        let name_len = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap()) as usize;
        let name = std::str::from_utf8(take(&mut pos, name_len)?)
            .map_err(|_| BackupError::Format("backup entry name is not valid UTF-8".into()))?
            .to_string();
        let data_len = u64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        if data_len > MAX_BACKUP_LEN {
            return Err(BackupError::Format(
                "backup entry is implausibly large".into(),
            ));
        }
        let data = take(&mut pos, data_len as usize)?.to_vec();
        out.push((name, data));
    }
    Ok(out)
}

/// Timestamp-named backup file path inside `dir`: `knot-backup-<millis>.knotbak`.
/// Millisecond Unix time keeps names chronological *and* lexically sortable, so
/// "newest first" is a plain reverse string sort with no calendar math.
fn backup_file_path(dir: &Path) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    dir.join(format!("knot-backup-{millis}.{BACKUP_EXT}"))
}

/// Pack the vault and write it to a fresh timestamped `.knotbak` in `dir`
/// (created if needed). Returns the path written.
pub fn create_backup(paths: &VaultPaths, dir: &Path) -> Result<PathBuf, BackupError> {
    let blob = pack(paths)?;
    std::fs::create_dir_all(dir)?;
    let file = backup_file_path(dir);
    std::fs::write(&file, &blob)?;
    Ok(file)
}

/// A validated restore, ready to commit. Produced by [`prepare_restore`]
/// (which does no writes) and applied by [`commit_restore`]. Splitting the two
/// lets the caller validate a chosen file *before* it tears down the live
/// session — so picking a non-backup file can't log the user out of a good
/// vault.
pub struct RestorePlan {
    /// One entry per vault slot: `Some(bytes)` to write, `None` to delete (a
    /// slot absent from the backup, e.g. `recovery.enc`). Owned, so the plan
    /// outlives the `VaultPaths` borrow.
    writes: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// The restored Argon2 salt — the caller mirrors it into `AppState::salt`,
    /// since the unlock path reads the cached salt, not the disk.
    pub salt: [u8; SALT_SIZE],
}

/// Read and validate a `.knotbak` against the vault layout, returning a
/// [`RestorePlan`] **without touching disk**. Only the four known vault file
/// names are honored — anything else in the container is dropped here, so a
/// tampered archive can't target a path outside the vault directory. The
/// required trio (`vault.db`, `vault.salt`, `dek.enc`) must be present and the
/// salt must be the right length.
pub fn prepare_restore(file: &Path, paths: &VaultPaths) -> Result<RestorePlan, BackupError> {
    let bytes = std::fs::read(file)?;
    if bytes.len() as u64 > MAX_BACKUP_LEN {
        return Err(BackupError::Format(
            "backup file is implausibly large".into(),
        ));
    }
    let parsed = unpack(&bytes)?;

    // Map each entry name to the vault slot it restores to; unknown names are
    // simply never added, so they can never be written.
    let mut writes: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for (path, _required) in entries(paths) {
        let name = file_name_of(path)?;
        let data = parsed
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| d.clone());
        writes.push((path.to_path_buf(), data));
    }

    // The required trio (db, salt, dek — indices 0..=2) must be present.
    for (path, data) in &writes[..3] {
        if data.is_none() {
            return Err(BackupError::Format(format!(
                "backup is missing {:?} — not a complete vault",
                path.file_name().unwrap_or_default()
            )));
        }
    }

    // The salt must be exactly SALT_SIZE bytes (it's mirrored into the cached
    // `AppState::salt`; a wrong length would corrupt the next unlock).
    let salt_bytes = writes[1].1.as_ref().unwrap();
    if salt_bytes.len() != SALT_SIZE {
        return Err(BackupError::Format(format!(
            "backup salt is {} bytes, expected {SALT_SIZE}",
            salt_bytes.len()
        )));
    }
    let mut salt = [0u8; SALT_SIZE];
    salt.copy_from_slice(salt_bytes);

    Ok(RestorePlan { writes, salt })
}

/// Apply a validated [`RestorePlan`], overwriting the live vault files. A slot
/// with no data (e.g. a backup that carried no `recovery.enc`) has its existing
/// file removed, so the restored vault doesn't keep a stale recovery wrapping
/// that no longer matches its DEK.
///
/// **Caller contract:** the live DB connection must already be closed and the
/// session locked-without-flush (see the module docs) before this runs.
pub fn commit_restore(plan: &RestorePlan) -> Result<(), BackupError> {
    if let Some(parent) = plan.writes.first().and_then(|(p, _)| p.parent()) {
        std::fs::create_dir_all(parent)?;
    }
    for (path, data) in &plan.writes {
        match data {
            Some(d) => write_atomic(path, d)?,
            None => {
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

/// `.knotbak` files in `dir`, newest first. Empty (not an error) when the dir
/// doesn't exist or holds none — the restore picker just shows nothing.
pub fn list_backups(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(BACKUP_EXT))
            .collect(),
        Err(_) => Vec::new(),
    };
    // File names embed millisecond time, so reverse lexical order = newest first.
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    files
}

/// Delete all but the newest `keep` backups in `dir`. `keep` is clamped to at
/// least 1 so rotation never wipes the backup just written. Best-effort: a
/// failed delete is reported but doesn't undo the rotation of the others.
pub fn rotate(dir: &Path, keep: usize) -> io::Result<()> {
    let keep = keep.max(1);
    let files = list_backups(dir); // newest first
    for stale in files.into_iter().skip(keep) {
        std::fs::remove_file(&stale)?;
    }
    Ok(())
}

/// Write `bytes` to `path` via a `.tmp` sibling + rename, so a reader (or a
/// crash) never sees a half-written file. Mirrors `storage`'s atomic writer.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ── UI: the backup & restore modal ───────────────────────────────────────────

/// Populate the "Backup & restore" dialog body. Opened as a nested modal from
/// the settings modal (mirrors [`crate::change_password::populate`]). Holds a
/// configurable backup folder + retention, a "Back up now" action, and a
/// "Restore…" action that picks a `.knotbak` and confirms before overwriting
/// the vault. `state` is needed only by the restore path (to lock the session).
pub fn populate(tree: &mut WidgetTree, dialog: usize, state: Rc<RefCell<AppState>>) {
    // In-modal feedback line — visible over the scrim, unlike a top-screen
    // notice which would sit behind it.
    let msg = Signal::new(String::new());
    let is_err = Signal::new(false);
    // Current folder: the configured path, or `None` = show the localized
    // "(default location)". A signal so "Change folder" updates it live.
    let folder = Signal::new(settings::configured_backup_dir());

    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::BackupTitle).to_string())
            .font_size(22.0)
            .color(settings::on_surface()),
    );
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::BackupDescription).to_string())
            .color(settings::on_surface_variant()),
    );

    // --- Backup folder ---
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::BackupFolderLabel).to_string())
            .color(settings::on_surface_variant()),
    );
    tree.add_child(
        dialog,
        TextWidget::reactive(move || match folder.get_clone() {
            Some(p) => p.to_string_lossy().into_owned(),
            None => i18n::tr(Key::BackupFolderDefault).to_string(),
        })
        .color(settings::on_surface()),
    );
    tree.add_child(
        dialog,
        Button::reactive_label(|| i18n::tr(Key::BackupChangeFolder).to_string())
            .radius(6.0)
            .on_click(move |_ctx| {
                if let Some(dir) = FileDialog::new()
                    .title(i18n::tr(Key::BackupChangeFolder))
                    .open_folder()
                {
                    settings::set_backup_dir(Some(dir.clone()));
                    folder.set(Some(dir));
                }
            }),
    );

    // --- Retention ---
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::BackupRetentionLabel).to_string())
            .color(settings::on_surface_variant()),
    );
    let retention = Signal::new(settings::backup_retention() as i64);
    tree.add_child(
        dialog,
        Input::new()
            .numeric()
            .min_value(1)
            .max_value(99)
            .number_value(retention)
            .on_change(move |_, _| settings::set_backup_retention(retention.get().max(1) as u32)),
    );

    // --- Automatic backup (on unlock, throttled) ---
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::BackupAutoLabel).to_string())
            .color(settings::on_surface_variant()),
    );
    let auto_sig = Signal::new(settings::auto_backup());
    let auto_row = tree.add_child(dialog, Container::row().gap(8.0));
    for choice in [AutoBackup::Off, AutoBackup::Daily, AutoBackup::Weekly] {
        let sig = auto_sig;
        let bg = Reactive::derive(move || {
            let t = settings::current_theme();
            if sig.get() == choice {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg = Reactive::derive(move || {
            let t = settings::current_theme();
            if sig.get() == choice {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        tree.add_child(
            auto_row,
            Button::reactive_label(move || i18n::tr(choice.key()).to_string())
                .radius(6.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |_ctx| {
                    sig.set(choice);
                    settings::set_auto_backup(choice);
                }),
        );
    }

    // --- Back up now ---
    let backup_state = Rc::clone(&state);
    tree.add_child(
        dialog,
        Button::reactive_label(|| i18n::tr(Key::BackupNowBtn).to_string())
            .radius(6.0)
            .on_click(move |_ctx| {
                // Flush any unsaved edits first so the snapshot reflects the
                // current vault, not just whatever the last auto-save tick
                // happened to land. Best-effort — a flush error still lets the
                // backup capture the on-disk state.
                let _ = backup_state.borrow_mut().flush_dirty();
                match run_backup() {
                    Ok(path) => {
                        msg.set(i18n::tr(Key::BackupSuccess).replace("{path}", &path));
                        is_err.set(false);
                    }
                    Err(e) => {
                        msg.set(format!("{}{e}", i18n::tr(Key::ErrBackupPrefix)));
                        is_err.set(true);
                    }
                }
            }),
    );

    // --- Restore ---
    let restore_state = Rc::clone(&state);
    tree.add_child(
        dialog,
        Button::reactive_label(|| i18n::tr(Key::BackupRestoreBtn).to_string())
            .radius(6.0)
            .on_click(move |ctx| {
                let Some(file) = FileDialog::new()
                    .title(i18n::tr(Key::DialogRestoreBackup))
                    .filter("Knot backup", &[BACKUP_EXT])
                    .open_file()
                else {
                    return;
                };
                // Confirm before overwriting — restore is destructive.
                let st = Rc::clone(&restore_state);
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column()
                        .width(420.0)
                        .padding(24.0)
                        .gap(16.0)
                        .background(settings::surface())
                        .radius(12.0),
                    move |tree, dialog| {
                        populate_restore_confirm(tree, dialog, Rc::clone(&st), file.clone())
                    },
                );
            }),
    );

    // --- Feedback line + Done ---
    tree.add_child(
        dialog,
        TextWidget::reactive(move || msg.get_clone()).color(Reactive::derive(move || {
            let t = settings::current_theme();
            if is_err.get() {
                t.colors.error
            } else {
                t.colors.success
            }
        })),
    );
    let done_row = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
    tree.add_child(
        done_row,
        Button::reactive_label(|| i18n::tr(Key::SettingsDone).to_string())
            .radius(6.0)
            .on_click(|ctx| ctx.pop_top_layer()),
    );
}

/// Pack the live vault into a fresh `.knotbak` in the resolved backup folder,
/// rotate old backups per the retention setting, record the time (so the
/// auto-backup throttle counts a manual backup too), and return the written
/// path as a display string. Rotation is best-effort — a failed prune doesn't
/// fail the backup that already landed.
fn run_backup() -> Result<String, BackupError> {
    let cfg_err = || BackupError::Format(i18n::tr(Key::ConfigUnavailable).to_string());
    let dir = settings::resolved_backup_dir().ok_or_else(cfg_err)?;
    let paths = VaultPaths::default_for_app().ok_or_else(cfg_err)?;
    let file = create_backup(&paths, &dir)?;
    let _ = rotate(&dir, settings::backup_retention() as usize);
    settings::set_last_backup_at(unix_now());
    Ok(file.to_string_lossy().into_owned())
}

/// Run an unlock-time backup if the configured cadence says one is due, then
/// record the time (via [`run_backup`]). Called once per successful unlock
/// (see `lock_screen`), so it never touches disk per frame. A no-op when
/// auto-backup is `Off` or the period hasn't elapsed; a failure is logged but
/// never blocks opening the vault.
pub fn maybe_auto_backup() {
    if !backup_due(
        settings::auto_backup(),
        settings::last_backup_at(),
        unix_now(),
    ) {
        return;
    }
    if let Err(e) = run_backup() {
        eprintln!("knot: auto-backup failed: {e}");
    }
}

/// Whether an automatic backup is due: enabled, and at least one period has
/// elapsed since the last backup. Pure (no clock / disk) so the cadence is
/// unit-testable. `last_at == 0` (never backed up) is always due when enabled.
fn backup_due(interval: AutoBackup, last_at: u64, now: u64) -> bool {
    match interval.period_secs() {
        None => false,
        // Never backed up — due immediately when enabled, regardless of clock.
        Some(_) if last_at == 0 => true,
        Some(period) => now.saturating_sub(last_at) >= period,
    }
}

/// Current Unix time in whole seconds (saturating to 0 before the epoch).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Populate the nested "Restore this backup?" confirmation. The destructive
/// commit only runs if the user confirms; a validation failure (wrong file)
/// shows here and leaves the live session untouched.
fn populate_restore_confirm(
    tree: &mut WidgetTree,
    dialog: usize,
    state: Rc<RefCell<AppState>>,
    file: PathBuf,
) {
    let msg = Signal::new(String::new());

    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::RestoreConfirmTitle).to_string())
            .font_size(20.0)
            .color(settings::on_surface()),
    );
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::RestoreConfirmBody).to_string())
            .color(settings::on_surface_variant()),
    );
    // Error feedback (only set on a validation failure; a success transitions
    // away before this could show).
    tree.add_child(
        dialog,
        TextWidget::reactive(move || msg.get_clone()).color(settings::error()),
    );

    let row = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
    tree.add_child(
        row,
        Button::reactive_label(|| i18n::tr(Key::RestoreCancel).to_string())
            .radius(6.0)
            .on_click(|ctx| ctx.pop_top_layer()),
    );
    let confirm_state = state;
    tree.add_child(
        row,
        Button::reactive_label(|| i18n::tr(Key::RestoreConfirmBtn).to_string())
            .radius(6.0)
            .on_click(move |ctx| {
                if let Err(e) = try_restore(&confirm_state, &file, ctx) {
                    msg.set(format!("{}{e}", i18n::tr(Key::ErrRestorePrefix)));
                }
            }),
    );
}

/// Validate, then (if valid) tear down the live session and overwrite the vault
/// from `file`, returning to the lock screen. An `Err` means validation failed
/// *before* the session was touched, so the caller can report it and the user
/// stays unlocked. Once validation passes the session is always torn down — a
/// rare write failure still lands on the lock screen (the session is gone), so
/// it's surfaced via a notice rather than returned.
fn try_restore(
    state: &Rc<RefCell<AppState>>,
    file: &Path,
    ctx: &mut EventContext,
) -> Result<(), BackupError> {
    let paths = VaultPaths::default_for_app()
        .ok_or_else(|| BackupError::Format(i18n::tr(Key::ConfigUnavailable).to_string()))?;

    // Validate with no writes — a bad pick must not lock the user out.
    let plan = prepare_restore(file, &paths)?;

    // Past here the session is gone no matter what: close the DB connection and
    // drop the in-memory notes WITHOUT flushing (which would clobber the file
    // we're about to restore).
    state.borrow_mut().discard_and_lock();

    match commit_restore(&plan) {
        Ok(()) => {
            // Refresh the cached salt the unlock path reads off `AppState`.
            state.borrow_mut().salt = plan.salt;
        }
        Err(e) => {
            // Disk failure mid-write (rare — validation already passed). The
            // session is already dropped, so press on to the lock screen and
            // surface the error; the user may need to retry or recover.
            eprintln!("knot: restore write failed: {e}");
            notice::show(format!("{}{e}", i18n::tr(Key::ErrRestorePrefix)));
        }
    }

    // Return to the lock screen. `ReplaceScreen` tears down every open layer
    // (this confirm dialog + the backup + settings modals), so no manual pops.
    let next = Rc::clone(state);
    ctx.replace_screen(move |tree| lock_screen::build(tree, Rc::clone(&next)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp dir for one test's vault + backups.
    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knot-backup-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paths_in(dir: &Path) -> VaultPaths {
        VaultPaths {
            db: dir.join("vault.db"),
            salt: dir.join("vault.salt"),
            dek: dir.join("dek.enc"),
            recovery: dir.join("recovery.enc"),
        }
    }

    /// Validate + commit a restore in one step, as the UI does back-to-back
    /// (minus the lock/relock dance, which is the caller's responsibility).
    fn restore(file: &Path, paths: &VaultPaths) -> Result<[u8; SALT_SIZE], BackupError> {
        let plan = prepare_restore(file, paths)?;
        commit_restore(&plan)?;
        Ok(plan.salt)
    }

    /// Write a complete fake vault (db/salt/dek/recovery) to `paths`.
    fn seed_vault(paths: &VaultPaths, with_recovery: bool) {
        std::fs::write(&paths.db, b"fake-sqlcipher-db").unwrap();
        std::fs::write(&paths.salt, [9u8; SALT_SIZE]).unwrap();
        std::fs::write(&paths.dek, b"wrapped-dek-blob").unwrap();
        if with_recovery {
            std::fs::write(&paths.recovery, b"wrapped-recovery-blob").unwrap();
        }
    }

    #[test]
    fn pack_unpack_round_trips_all_files() {
        let dir = tmp_dir("roundtrip");
        let paths = paths_in(&dir);
        seed_vault(&paths, true);

        let blob = pack(&paths).unwrap();
        let entries = unpack(&blob).unwrap();
        assert_eq!(entries.len(), 4, "db + salt + dek + recovery");

        let find = |name: &str| entries.iter().find(|(n, _)| n == name).map(|(_, d)| d);
        assert_eq!(find("vault.db").unwrap(), b"fake-sqlcipher-db");
        assert_eq!(find("dek.enc").unwrap(), b"wrapped-dek-blob");
        assert_eq!(find("recovery.enc").unwrap(), b"wrapped-recovery-blob");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pack_skips_absent_recovery() {
        let dir = tmp_dir("no-recovery");
        let paths = paths_in(&dir);
        seed_vault(&paths, false);

        let entries = unpack(&pack(&paths).unwrap()).unwrap();
        assert_eq!(entries.len(), 3, "recovery.enc is optional");
        assert!(entries.iter().all(|(n, _)| n != "recovery.enc"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_then_restore_into_a_fresh_dir() {
        let src = tmp_dir("src");
        let src_paths = paths_in(&src);
        seed_vault(&src_paths, true);

        let backup_dir = tmp_dir("backups");
        let file = create_backup(&src_paths, &backup_dir).unwrap();
        assert!(file.exists());

        // Restore into a *different*, empty vault directory.
        let dst = tmp_dir("dst");
        let dst_paths = paths_in(&dst);
        let salt = restore(&file, &dst_paths).unwrap();

        assert_eq!(salt, [9u8; SALT_SIZE], "restore returns the packed salt");
        assert_eq!(std::fs::read(&dst_paths.db).unwrap(), b"fake-sqlcipher-db");
        assert_eq!(std::fs::read(&dst_paths.dek).unwrap(), b"wrapped-dek-blob");
        assert_eq!(
            std::fs::read(&dst_paths.recovery).unwrap(),
            b"wrapped-recovery-blob"
        );

        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&backup_dir).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn restore_drops_stale_recovery_when_backup_has_none() {
        // Backup made without a recovery wrapping; restoring over a vault that
        // *had* one must remove the stale recovery.enc (it no longer matches).
        let src = tmp_dir("src-norec");
        let src_paths = paths_in(&src);
        seed_vault(&src_paths, false);
        let file = create_backup(&src_paths, &src).unwrap();

        let dst = tmp_dir("dst-hadrec");
        let dst_paths = paths_in(&dst);
        seed_vault(&dst_paths, true);
        assert!(dst_paths.recovery.exists());

        restore(&file, &dst_paths).unwrap();
        assert!(
            !dst_paths.recovery.exists(),
            "a recovery-less backup must clear a stale recovery.enc"
        );

        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn restore_rejects_bad_magic() {
        let dir = tmp_dir("badmagic");
        let bad = dir.join("x.knotbak");
        std::fs::write(&bad, b"not-a-knot-backup-at-all").unwrap();
        let err = restore(&bad, &paths_in(&dir)).unwrap_err();
        assert!(matches!(err, BackupError::Format(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_rejects_incomplete_vault() {
        // A container that parses but is missing the required db file.
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&1u32.to_le_bytes()); // one entry
        let name = b"vault.salt";
        blob.extend_from_slice(&(name.len() as u32).to_le_bytes());
        blob.extend_from_slice(name);
        blob.extend_from_slice(&(SALT_SIZE as u64).to_le_bytes());
        blob.extend_from_slice(&[0u8; SALT_SIZE]);

        let dir = tmp_dir("incomplete");
        let file = dir.join("partial.knotbak");
        std::fs::write(&file, &blob).unwrap();
        let err = restore(&file, &paths_in(&dir)).unwrap_err();
        assert!(matches!(err, BackupError::Format(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_entry_names_are_ignored_not_written() {
        // A hostile container naming a file outside the vault must not cause a
        // write there; only the four known slots are ever touched.
        let dir = tmp_dir("evil");
        let paths = paths_in(&dir);
        seed_vault(&paths, false);
        let mut blob = pack(&paths).unwrap();
        // Bump the count and append an "evil.txt" entry by hand.
        // (Rebuild from scratch is simpler than patching the count in place.)
        let mut evil = Vec::new();
        evil.extend_from_slice(MAGIC);
        evil.extend_from_slice(&4u32.to_le_bytes());
        // re-emit the 3 real entries from the parsed blob...
        let real = unpack(&blob).unwrap();
        for (name, data) in &real {
            evil.extend_from_slice(&(name.len() as u32).to_le_bytes());
            evil.extend_from_slice(name.as_bytes());
            evil.extend_from_slice(&(data.len() as u64).to_le_bytes());
            evil.extend_from_slice(data);
        }
        let evil_name = "evil.txt";
        evil.extend_from_slice(&(evil_name.len() as u32).to_le_bytes());
        evil.extend_from_slice(evil_name.as_bytes());
        evil.extend_from_slice(&3u64.to_le_bytes());
        evil.extend_from_slice(b"pwn");
        blob = evil;

        let dst = tmp_dir("evil-dst");
        let dst_paths = paths_in(&dst);
        let file = dst.join("evil.knotbak");
        std::fs::write(&file, &blob).unwrap();
        restore(&file, &dst_paths).unwrap();

        assert!(
            !dst.join("evil.txt").exists(),
            "an unknown entry name must never be written to disk"
        );
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn rotate_keeps_only_the_newest_n() {
        let dir = tmp_dir("rotate");
        // Create files with ascending millisecond names by hand (the helper's
        // 1ms granularity could collide in a tight loop).
        for ms in [1000u64, 2000, 3000, 4000, 5000] {
            std::fs::write(dir.join(format!("knot-backup-{ms}.knotbak")), b"x").unwrap();
        }
        rotate(&dir, 2).unwrap();

        let left = list_backups(&dir);
        assert_eq!(left.len(), 2, "keep=2 leaves two");
        // Newest first: 5000 then 4000.
        assert!(left[0].to_string_lossy().contains("5000"));
        assert!(left[1].to_string_lossy().contains("4000"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_due_respects_cadence() {
        const DAY: u64 = 24 * 60 * 60;
        // Off is never due, however long it's been.
        assert!(!backup_due(AutoBackup::Off, 0, 10 * DAY));
        // Never backed up (last_at = 0) → due as soon as it's enabled.
        assert!(backup_due(AutoBackup::Daily, 0, 1));
        // Daily: not due before a day, due at exactly a day.
        assert!(!backup_due(AutoBackup::Daily, 1000, 1000 + DAY - 1));
        assert!(backup_due(AutoBackup::Daily, 1000, 1000 + DAY));
        // Weekly: a single day isn't enough; a week is.
        assert!(!backup_due(AutoBackup::Weekly, 1000, 1000 + DAY));
        assert!(backup_due(AutoBackup::Weekly, 1000, 1000 + 7 * DAY));
        // A clock that went backwards (now < last) saturates, never spuriously due.
        assert!(!backup_due(AutoBackup::Daily, 5000, 1000));
    }

    #[test]
    fn rotate_keep_is_clamped_to_at_least_one() {
        let dir = tmp_dir("rotate-zero");
        std::fs::write(dir.join("knot-backup-1.knotbak"), b"x").unwrap();
        rotate(&dir, 0).unwrap();
        assert_eq!(
            list_backups(&dir).len(),
            1,
            "keep=0 must not wipe the only backup"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
