//! Knot M2 — SQLCipher-backed multi-note editor.
//!
//! Promoted from M1 by adding persistence: notes now live in a
//! SQLCipher 4 database at `<config>/knot/vault.db`, page-encrypted
//! under the Argon2-derived master key. Each note row also carries an
//! XChaCha20-Poly1305 ciphertext (the existing M1 crypto), so the two
//! layers cover each other.
//!
//! Lifecycle:
//!   * First launch (no `vault.db` yet): seed the DB with the demo
//!     notes under [`DEMO_PASSWORD`], drop the user straight into the
//!     vault. M3 will replace this with a proper setup screen.
//!   * Subsequent launch: lock screen prompts for the master password,
//!     opens the SQLCipher DB on success.
//!   * While unlocked: an auto-save tick (`on_frame`) flushes dirty
//!     notes every `tick_interval` (default 500 ms). Lock also runs a
//!     belt-and-suspenders full rewrite before dropping the key.

mod crypto;
mod editor;
mod lock_screen;
mod sidebar;
mod state;
mod storage;
mod vault_screen;

use std::cell::RefCell;
use std::rc::Rc;

use shroud::app::App;
use shroud::widgets::tree::WidgetTree;

use crate::crypto::{derive_key, random_salt};
use crate::state::{AppState, Note, NoteId, Phase};
use crate::storage::{StorageError, VaultPaths, VaultStorage};

pub const DEMO_PASSWORD: &str = "knot-demo-2026";

const DEMO_NOTES: &[(&str, &str)] = &[
    (
        "Welcome to Knot",
        "Knot is a privacy-first encrypted notes app. M2 added SQLCipher\n\
         persistence — your notes survive a relaunch now.\n\
         \n\
         Try editing this note; the body is auto-saved every ~500ms while\n\
         you type, and the lock button does a full re-encrypt + flush\n\
         before dropping the master key.",
    ),
    (
        "Roadmap",
        "M1: in-memory vault, plain-text editor, sidebar add / delete\n\
         M2 (this build): SQLCipher persistence under <config>/knot/vault.db\n\
         M3+: setup screen, BIP39 recovery, settings persistence, themes\n\
         \n\
         If you delete the DB file the next launch reseeds these demo\n\
         notes under the demo password.",
    ),
    (
        "日本語テスト",
        "Knot は日本語のメモも普通に扱えます。Phase 39 / 40 で IME 直接\n\
         入力と候補ウィンドウ追従も動くようになったので、普通の文章を\n\
         書く時の体験が一段マシになっています。\n\
         \n\
         このメモを編集して保存 → ロック → 解錠 すると、復号往復が本当に\n\
         走っていることが確認できる(中身が残っていれば成功)。",
    ),
];

fn main() {
    App::new()
        .title("Knot \u{2014} M2")
        .size(1080, 720)
        .capture_prevention(true)
        .run(|scope| {
            let state = init_state();

            // Auto-save tick — re-runs every `App::tick_interval`
            // (default 500 ms) for as long as a frame hook is set.
            // Cheap when nothing's dirty (early-returns inside
            // flush_dirty), so leaving it always-on costs nothing.
            let state_for_tick = Rc::clone(&state);
            scope.on_frame(move || {
                if let Err(e) = state_for_tick.borrow_mut().flush_dirty() {
                    eprintln!("knot: auto-save tick failed: {}", e);
                }
            });

            let starts_unlocked = matches!(state.borrow().phase, Phase::Unlocked { .. });
            let mut tree = WidgetTree::new();
            if starts_unlocked {
                vault_screen::build(&mut tree, state);
            } else {
                lock_screen::build(&mut tree, state);
            }
            tree
        });
}

/// Resolve the on-disk vault state into an `AppState`.
///
/// First launch (no `vault.db`) seeds a fresh vault under
/// [`DEMO_PASSWORD`] and starts the app unlocked so the user can see
/// the demo notes. Subsequent launches read the salt file and return
/// a `Locked` state for the lock screen to handle. Persistence
/// failures are surfaced via `panic!` rather than silently falling
/// back to in-memory mode — Knot's whole point is durable storage,
/// and pretending otherwise hides a real misconfiguration from the
/// user.
fn init_state() -> Rc<RefCell<AppState>> {
    let paths = VaultPaths::default_for_app()
        .expect("knot: OS config directory unavailable — cannot run without persistence");

    if paths.vault_exists() {
        let salt = paths
            .read_salt()
            .unwrap_or_else(|e| panic!("knot: failed to read vault.salt: {}", e));
        Rc::new(RefCell::new(AppState::new_locked(salt, 1)))
    } else {
        seed_fresh_vault(&paths)
            .unwrap_or_else(|e| panic!("knot: failed to seed fresh vault: {}", e))
    }
}

/// First-launch path: generate salt, write it, derive the demo key,
/// open a freshly-keyed SQLCipher DB, and seed it with [`DEMO_NOTES`].
/// Returns an Unlocked `AppState` so the user lands in the vault
/// screen directly — no lock-then-unlock dance for the very first
/// run (the demo password is shown in the lock screen hint anyway).
fn seed_fresh_vault(paths: &VaultPaths) -> Result<Rc<RefCell<AppState>>, StorageError> {
    let salt = random_salt();
    paths.write_salt(&salt)?;
    let key = derive_key(DEMO_PASSWORD.as_bytes(), &salt);
    let storage = VaultStorage::open(&paths.db, &key)?;

    let initial_notes: Vec<Note> = DEMO_NOTES
        .iter()
        .enumerate()
        .map(|(i, (t, b))| Note {
            id: (i + 1) as NoteId,
            title: (*t).to_string(),
            body: (*b).to_string(),
        })
        .collect();

    let mut state = AppState::new_locked(salt, (initial_notes.len() as NoteId) + 1);
    state.become_unlocked(key, initial_notes, storage);
    // become_unlocked records the resident plaintext + opens the storage,
    // but the seed rows aren't on disk yet — push them now so a crash
    // before the first auto-save tick still finds the demo set on the
    // next launch.
    state.rewrite_vault_to_storage()?;

    Ok(Rc::new(RefCell::new(state)))
}
