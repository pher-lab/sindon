//! password_manager — validation example for sindon.
//!
//! Two-screen password manager wired through the Phase 18c-1 tree-mutation
//! primitives, the Phase 18c-2 add-entry flow, and the Phase 18d reactive
//! value / clear-trigger bindings. The demo exists to stress sindon's
//! secret-handling story end-to-end:
//!
//! - `SecureInput` → master password stays inside `SecureString`; no
//!   `String` intermediary. A `ClearTrigger` (Phase 18d) zeroizes the
//!   buffer after Save.
//! - argon2id key derivation from the typed master password.
//! - chacha20poly1305 decrypt of each entry's password into a `SecureString`.
//! - `SecureClipboard::write_secure` + 10 s auto-clear, driven by
//!   `AppScope::on_frame`.
//! - `EventContext::replace_screen` → lock ⇄ vault transitions. The old
//!   tree drops in full, so the `SecureInput`'s `SecureString` zeroizes as
//!   part of the transition (gap #1 validation for Phase 18c-1).
//! - `Input::value(Signal<String>)` → the add-entry drafts live in signals
//!   that the form inputs bind bidirectionally. Clearing is `signal.set("")`
//!   rather than rebuilding the form subtree (gap #7 validation for 18d).
//! - `EventContext::rebuild_children` → only the list subtree is rebuilt
//!   on save; the form stays put and its inputs clear via the signals.
//! - Screen capture prevention (Windows only — no-op elsewhere).
//!
//! Out of scope: disk persistence, delete-entry, search.
//!
//! Hardcoded demo master password: `hunter2`.

use std::cell::RefCell;
use std::rc::Rc;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroizing;

use sindon::app::App;
use sindon::platform::SecureClipboard;
use sindon::reactive::Signal;
use sindon::security::SecureString;
use sindon::widgets::shortcut::Shortcut;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, ClearTrigger, Container, Input, SecureInput, TextWidget};

const DEMO_PASSWORD: &str = "hunter2";

struct EncryptedEntry {
    site: String,
    username: String,
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

/// Decrypted row — site/username stay plaintext, only the secret payload
/// lives in a `SecureString` so it zeroizes on relock.
struct UnlockedEntry {
    password: SecureString,
}

enum VaultState {
    Locked,
    Unlocked(Vec<UnlockedEntry>),
    /// Set after a failed unlock.
    Error(String),
}

struct AppState {
    salt: [u8; 16],
    encrypted: Vec<EncryptedEntry>,
    state: VaultState,
    clipboard: SecureClipboard,
    /// Master key resident while unlocked; wiped on lock. Kept so the
    /// add-entry flow can encrypt new rows without re-prompting for the
    /// master password.
    key: Option<Zeroizing<[u8; 32]>>,
    /// Add-entry drafts, bound bidirectionally to the form's `Input`
    /// widgets via Phase-18d `.value(Signal<String>)`. Save reads the
    /// current values, then writes `""` back to clear the inputs without a
    /// subtree rebuild.
    draft_site: Signal<String>,
    draft_username: Signal<String>,
    /// Fired after each successful Save to zeroize the SecureInput buffer
    /// holding the freshly-entered password.
    clear_password: ClearTrigger,
    /// Stable tree index for the vault screen's list container. `Some`
    /// while the vault screen is mounted, `None` on the lock screen. Save
    /// reads this to target `rebuild_children` for the list only — the
    /// form stays put and clears through the reactive bindings.
    list_idx: Option<usize>,
    /// Tree index of the add-entry "Site" Input. Set in `build_add_form`,
    /// cleared on relock. Ctrl+N (the new-entry shortcut) reads it to jump
    /// focus into the field without disturbing scroll or list state.
    site_input_idx: Option<usize>,
}

impl AppState {
    fn clear_vault_refs(&mut self) {
        self.list_idx = None;
        self.site_input_idx = None;
        self.draft_site.set(String::new());
        self.draft_username.set(String::new());
        self.clear_password.bump();
    }
}

/// Argon2id key derivation. Parameters tuned for interactive login on
/// modest hardware (~50 ms on a recent laptop). RFC 9106 recommends
/// m=19 MiB, t=2, p=1 for the "low-memory" profile; we follow that.
fn derive_key(password: &[u8], salt: &[u8]) -> Zeroizing<[u8; 32]> {
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19_456, 2, 1, Some(32)).expect("valid argon2 params"),
    );
    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(password, salt, key.as_mut())
        .expect("argon2 kdf");
    key
}

/// Seed an in-memory vault encrypted under the demo master password. The
/// ciphertext would normally be loaded from disk.
fn make_demo_vault() -> ([u8; 16], Vec<EncryptedEntry>) {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(DEMO_PASSWORD.as_bytes(), &salt);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));

    let demo = [
        (
            "github.com",
            "alice@example.com",
            "correct_horse_battery_staple",
        ),
        ("aws.amazon.com", "alice-ops", "AKIA_demo_secret_key"),
        ("bank.example", "alice", "Pa55w0rd!"),
    ];

    let entries = demo
        .iter()
        .map(|(site, user, pass)| {
            let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
            let ciphertext = cipher
                .encrypt(&nonce, pass.as_bytes())
                .expect("encrypt demo entry");
            EncryptedEntry {
                site: (*site).to_string(),
                username: (*user).to_string(),
                nonce: nonce.into(),
                ciphertext,
            }
        })
        .collect();

    (salt, entries)
}

/// Derive the master key from the entered password and try to decrypt every
/// row. Any AEAD tag failure flips state to `Error`. On success, decrypted
/// passwords land in `UnlockedEntry`s backed by `SecureString`, and the
/// derived key stays resident in `AppState::key` for add-entry encryption.
fn try_unlock(state: &Rc<RefCell<AppState>>, master: &SecureString) {
    let mut s = state.borrow_mut();
    let key = master.expose(|m| derive_key(m.as_bytes(), &s.salt));
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));

    let mut unlocked = Vec::with_capacity(s.encrypted.len());
    for e in &s.encrypted {
        match cipher.decrypt(Nonce::from_slice(&e.nonce), e.ciphertext.as_ref()) {
            Ok(pt) => {
                let mut pt = Zeroizing::new(pt);
                let password = match std::str::from_utf8(&pt) {
                    Ok(s) => SecureString::new(s),
                    Err(_) => {
                        s.state = VaultState::Error("corrupt entry".into());
                        s.key = None;
                        return;
                    }
                };
                pt.as_mut_slice().fill(0);
                unlocked.push(UnlockedEntry { password });
            }
            Err(_) => {
                s.state = VaultState::Error("wrong master password".into());
                s.key = None;
                return;
            }
        }
    }
    s.state = VaultState::Unlocked(unlocked);
    s.key = Some(key);
}

/// Encrypt a new entry with the currently-resident key and append it to
/// both the ciphertext store and the unlocked view. Returns `false` if
/// preconditions aren't met (locked, empty site, etc.) so callers can skip
/// the ensuing rebuild. On success, clears the draft signals and bumps the
/// SecureInput clear trigger so the form zeroes out without a rebuild.
fn add_entry(state: &Rc<RefCell<AppState>>, password: &SecureString) -> bool {
    let mut s = state.borrow_mut();
    let site = s.draft_site.get_clone();
    if site.trim().is_empty() {
        return false;
    }
    let username = s.draft_username.get_clone();
    let Some(key) = s.key.as_ref() else {
        return false;
    };
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = match password.expose(|raw| cipher.encrypt(&nonce, raw.as_bytes())) {
        Ok(ct) => ct,
        Err(_) => return false,
    };

    s.encrypted.push(EncryptedEntry {
        site,
        username,
        nonce: nonce.into(),
        ciphertext,
    });
    // Mirror into the unlocked view so Copy works immediately for the new
    // row without a relock round-trip.
    if let VaultState::Unlocked(v) = &mut s.state {
        let pw_clone = password.expose(SecureString::new);
        v.push(UnlockedEntry { password: pw_clone });
    }

    // Clear the form through its reactive bindings. Signals fire on the
    // next paint for the two Inputs; the SecureInput reads `clear_password`
    // in its event loop (we're currently inside `on_submit`) and zeroizes
    // its buffer before the next render.
    s.draft_site.set(String::new());
    s.draft_username.set(String::new());
    s.clear_password.bump();
    true
}

/// Lock screen — master password prompt. On successful unlock, hands off
/// to `build_vault_screen` via `ctx.replace_screen`, which drops the whole
/// lock subtree (including the `SecureInput` holding the typed password)
/// and zeroizes it.
fn build_lock_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    state.borrow_mut().clear_vault_refs();

    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .gap(12.0),
    );

    tree.add_child(
        root,
        TextWidget::new("sindon \u{2014} Password Manager").font_size(24.0),
    );
    tree.add_child(
        root,
        TextWidget::new(format!("Master password (hint: {}):", DEMO_PASSWORD)),
    );

    let unlock_state = Rc::clone(&state);
    tree.add_child(
        root,
        SecureInput::new()
            .placeholder("Enter master password, press Enter to unlock")
            .on_submit(move |master, ctx| {
                try_unlock(&unlock_state, master);
                // On success, hand over to the vault screen. The replace
                // tears down this screen's `SecureInput`, so the typed
                // password zeroizes as part of the transition.
                let unlocked = matches!(unlock_state.borrow().state, VaultState::Unlocked(_));
                if unlocked {
                    let next = Rc::clone(&unlock_state);
                    ctx.replace_screen(move |tree| build_vault_screen(tree, next));
                }
            }),
    );

    let status_state = Rc::clone(&state);
    tree.add_child(
        root,
        TextWidget::reactive(move || match &status_state.borrow().state {
            VaultState::Locked => "Locked.".into(),
            VaultState::Error(e) => format!("Locked \u{2014} {}", e),
            // The handler transitions away on success, so this arm is
            // only ever on screen for a single frame.
            VaultState::Unlocked(_) => "Unlocked \u{2014} opening vault\u{2026}".into(),
        })
        .color(sindon::core::Color::rgb(0.7, 0.7, 0.75)),
    );
}

/// Vault screen — entry list with Copy buttons, an add-entry form, and a
/// Lock button. Lock drops the unlocked entries (zeroizing each
/// `SecureString`) and swaps back to the lock screen.
fn build_vault_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .gap(12.0),
    );

    tree.add_child(
        root,
        TextWidget::new("sindon \u{2014} Password Manager").font_size(24.0),
    );

    let lock_state = Rc::clone(&state);
    tree.add_child(
        root,
        Button::new("Lock").on_click(move |ctx| {
            // Clearing state first drops the `Vec<UnlockedEntry>` + key,
            // which zeroizes every secret held in memory. Doing this
            // before queuing the screen swap keeps the window between
            // "locked in data" and "locked in UI" as narrow as possible.
            {
                let mut s = lock_state.borrow_mut();
                s.state = VaultState::Locked;
                s.key = None;
            }
            let next = Rc::clone(&lock_state);
            ctx.replace_screen(move |tree| build_lock_screen(tree, next));
        }),
    );

    let status_state = Rc::clone(&state);
    tree.add_child(
        root,
        TextWidget::reactive(move || {
            let s = status_state.borrow();
            let remaining = s.clipboard.time_remaining();
            match &s.state {
                VaultState::Unlocked(v) => {
                    // Ceiling the remaining duration so each displayed
                    // number is visible for the same ~1 s window, even
                    // though ticks land mid-second.
                    let tail = remaining
                        .map(|d| {
                            let secs = d.as_secs_f64().ceil() as u64;
                            format!(" \u{2014} clipboard clears in {}s", secs)
                        })
                        .unwrap_or_default();
                    format!("Unlocked \u{2014} {} entries{}", v.len(), tail)
                }
                // Only observable during the single frame between clicking
                // Lock and the replace_screen draining.
                _ => "Locking\u{2026}".into(),
            }
        })
        .color(sindon::core::Color::rgb(0.7, 0.7, 0.75)),
    );

    // Add-entry form. Builds once and stays put across Saves — its Inputs
    // are bound to the draft signals, and the SecureInput watches the
    // clear trigger, so "reset after Save" is purely signal-driven.
    let form = tree.add_child(root, Container::column().gap(6.0).padding(8.0));
    build_add_form(tree, form, Rc::clone(&state));

    // List container — rebuilt by Save whenever a new entry lands.
    let list = tree.add_child(root, Container::column().gap(6.0));
    state.borrow_mut().list_idx = Some(list);
    build_list_rows(tree, list, Rc::clone(&state));
}

fn build_add_form(tree: &mut WidgetTree, parent: usize, state: Rc<RefCell<AppState>>) {
    let (site_sig, user_sig, clear) = {
        let s = state.borrow();
        (s.draft_site, s.draft_username, s.clear_password)
    };

    tree.add_child(
        parent,
        TextWidget::new("Add entry").color(sindon::core::Color::rgb(0.7, 0.7, 0.75)),
    );

    let site_idx = tree.add_child(parent, Input::new().placeholder("Site").value(site_sig));
    state.borrow_mut().site_input_idx = Some(site_idx);
    tree.add_child(parent, Input::new().placeholder("Username").value(user_sig));

    // Submit is driven by Enter in the password field. The handler
    // encrypts the draft, appends it, and rebuilds just the list subtree.
    // The form clears through the draft signals + clear trigger.
    let save_state = Rc::clone(&state);
    tree.add_child(
        parent,
        SecureInput::new()
            .placeholder("Password (press Enter to save)")
            .clear_on(clear)
            .on_submit(move |pw, ctx| {
                if !add_entry(&save_state, pw) {
                    return;
                }
                let list_idx = save_state.borrow().list_idx;
                if let Some(list_idx) = list_idx {
                    let s = Rc::clone(&save_state);
                    ctx.rebuild_children(list_idx, move |tree, p| build_list_rows(tree, p, s));
                }
            }),
    );
}

fn build_list_rows(tree: &mut WidgetTree, parent: usize, state: Rc<RefCell<AppState>>) {
    let entry_count = state.borrow().encrypted.len();

    for idx in 0..entry_count {
        let row = tree.add_child(parent, Container::row().gap(12.0).padding(8.0));

        let site_state = Rc::clone(&state);
        tree.add_child(
            row,
            TextWidget::reactive(move || {
                site_state
                    .borrow()
                    .encrypted
                    .get(idx)
                    .map(|e| e.site.clone())
                    .unwrap_or_default()
            }),
        );

        let user_state = Rc::clone(&state);
        tree.add_child(
            row,
            TextWidget::reactive(move || {
                user_state
                    .borrow()
                    .encrypted
                    .get(idx)
                    .map(|e| e.username.clone())
                    .unwrap_or_default()
            }),
        );

        let click_state = Rc::clone(&state);
        tree.add_child(
            row,
            Button::new("Copy").on_click(move |_ctx| {
                let mut s = click_state.borrow_mut();
                // Split the borrow so we can read `state` while mutating
                // `clipboard` on the same struct.
                let AppState {
                    state, clipboard, ..
                } = &mut *s;
                if let VaultState::Unlocked(entries) = state {
                    if let Some(e) = entries.get(idx) {
                        let _ = clipboard.write_secure(&e.password);
                    }
                }
            }),
        );
    }
}

fn main() {
    App::new()
        .title("sindon \u{2014} Password Manager")
        .size(640, 560)
        .capture_prevention(true)
        .run(|scope| {
            let (salt, encrypted) = make_demo_vault();

            // Signals and ClearTrigger need to be constructed inside the
            // app callback so they register with the thread-local reactive
            // runtime that `App::run` just initialized.
            let state = Rc::new(RefCell::new(AppState {
                salt,
                encrypted,
                state: VaultState::Locked,
                clipboard: SecureClipboard::new(),
                key: None,
                draft_site: Signal::new(String::new()),
                draft_username: Signal::new(String::new()),
                clear_password: ClearTrigger::new(),
                list_idx: None,
                site_input_idx: None,
            }));

            // Per-frame tick — advances the clipboard auto-clear timer on
            // the UI thread. `tick_interval` (default 500 ms) drives the
            // idle cadence so the countdown refreshes even when the user
            // is idle. Outlives any single screen.
            let tick_state = Rc::clone(&state);
            scope.on_frame(move |_ctx| {
                tick_state.borrow_mut().clipboard.tick();
            });

            // Ctrl+L — lock from anywhere. Global scope so it fires even
            // while the user is typing into a draft Input. Idempotent on
            // the lock screen (already locked, key already cleared).
            let lock_state = Rc::clone(&state);
            scope.on_shortcut(Shortcut::ctrl('l'), move |ctx| {
                let was_unlocked = {
                    let mut s = lock_state.borrow_mut();
                    let was = matches!(s.state, VaultState::Unlocked(_));
                    if was {
                        s.state = VaultState::Locked;
                        s.key = None;
                    }
                    was
                };
                if was_unlocked {
                    let next = Rc::clone(&lock_state);
                    ctx.event_ctx
                        .replace_screen(move |tree| build_lock_screen(tree, next));
                }
            });

            // Ctrl+N — jump focus to the Site field on the vault screen.
            // Default scope (`WhenNoTextInput`) so Ctrl+N inside the
            // password field or any other Input is silently dropped
            // instead of yanking focus mid-typing.
            let focus_state = Rc::clone(&state);
            scope.on_shortcut(Shortcut::ctrl('n'), move |ctx| {
                if let Some(idx) = focus_state.borrow().site_input_idx {
                    ctx.event_ctx.focus(idx);
                }
            });

            let mut tree = WidgetTree::new();
            build_lock_screen(&mut tree, state);
            tree
        });
}
