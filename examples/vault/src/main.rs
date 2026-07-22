//! vault — shroud's second grounding app, and a forcing function for list
//! virtualization.
//!
//! Knot (the first grounding) handles *few large* secret documents you edit in
//! place. A credential vault is the opposite secret shape: *many small,
//! high-churn* secrets you copy without ever revealing. That shape is what
//! forced the one release-blocking framework gap Knot never did — a plain
//! [`ScrollView`](shroud::widgets::ScrollView) instantiates *all* its children,
//! so a vault of hundreds of entries lays out every row every frame. The list
//! here is a [`VirtualList`](shroud::widgets::VirtualList), which materializes
//! only the visible window; `VAULT_PLAIN=1` switches back to the plain
//! `ScrollView` as an A/B baseline (run either with `SHROUD_PERF=1` to feel it).
//!
//! Secret handling mirrors Knot: the master password never leaves a
//! `SecureString`; the SQLCipher DB is page-encrypted under the Argon2-derived
//! key and each entry's secret is additionally sealed per-row; secrets are
//! decrypted into `SecureString`s only while unlocked (relock drops them, which
//! zeroizes); Copy routes through [`SecureClipboard`] with a 10 s auto-clear.
//!
//! First run seeds [`SEED_COUNT`] entries under the demo password and persists
//! them; later runs load from disk. Demo master password: `hunter2`.

mod storage;

use std::cell::RefCell;
use std::rc::Rc;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroizing;

use shroud::app::App;
use shroud::core::Color;
use shroud::platform::SecureClipboard;
use shroud::security::SecureString;
use shroud::widgets::shortcut::Shortcut;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, ScrollView, TextWidget, VirtualList};

use storage::{StoredEntry, VaultPaths, VaultStorage};

/// Demo master password, baked in like `password_manager` — this is a framework
/// validation example, not a shipping product.
const DEMO_PASSWORD: &str = "hunter2";

/// How many entries to seed on first run. Large enough that a plain `ScrollView`
/// (which materializes every child) visibly stutters — the wall this example
/// exists to knock down.
const SEED_COUNT: usize = 1000;

/// Full row pitch for the virtualized list (a row's box is exactly this tall,
/// padding included), and the height the plain rows match for a fair A/B.
const ROW_H: f32 = 44.0;

const BG: Color = Color::rgb(0.10, 0.11, 0.15);
const PANEL: Color = Color::rgb(0.13, 0.14, 0.19);
const HEADING: Color = Color::rgb(0.92, 0.94, 1.0);
const MUTED: Color = Color::rgb(0.70, 0.72, 0.80);
const ROW: Color = Color::rgb(0.16, 0.17, 0.23);

/// A decrypted entry, live only while the vault is unlocked. Site/username are
/// plaintext (not the secret); the secret lives in a `SecureString` that
/// zeroizes when the `Unlocked` vec is dropped on relock.
struct UnlockedEntry {
    site: String,
    username: String,
    password: SecureString,
}

enum VaultState {
    Locked,
    Unlocked(Vec<UnlockedEntry>),
    /// Set after a failed unlock attempt.
    Error(String),
}

struct AppState {
    paths: VaultPaths,
    salt: [u8; storage::SALT_SIZE],
    state: VaultState,
    clipboard: SecureClipboard,
}

/// Argon2id key derivation (RFC 9106 low-memory profile: m=19 MiB, t=2, p=1),
/// matching `password_manager`.
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

/// Seal `plaintext` under `key` with a fresh nonce.
fn seal(key: &[u8; 32], plaintext: &[u8]) -> ([u8; storage::NONCE_SIZE], Vec<u8>) {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext).expect("seal");
    (nonce.into(), ciphertext)
}

/// Ensure a vault exists on disk, returning its salt. On first run: generate a
/// salt, seal [`SEED_COUNT`] entries under the demo password, and persist. On
/// later runs: just read the stored salt back.
fn ensure_vault(paths: &VaultPaths) -> [u8; storage::SALT_SIZE] {
    if paths.exists() {
        return paths.read_salt().expect("read vault salt");
    }

    let mut salt = [0u8; storage::SALT_SIZE];
    OsRng.fill_bytes(&mut salt);
    let key = derive_key(DEMO_PASSWORD.as_bytes(), &salt);

    let entries: Vec<StoredEntry> = (0..SEED_COUNT)
        .map(|i| {
            let secret = format!("pw-{i:04}-correct-horse-battery");
            let (nonce, ciphertext) = seal(&key, secret.as_bytes());
            StoredEntry {
                id: i as i64,
                site: format!("service{i:04}.example.com"),
                username: format!("user{i:04}@example.com"),
                nonce,
                ciphertext,
            }
        })
        .collect();

    let mut store = VaultStorage::open(&paths.db, &key).expect("create vault db");
    store.save_all(&entries).expect("seed vault");
    paths.write_salt(&salt).expect("write vault salt");
    salt
}

/// Derive the key from the entered password, open the DB (a `BadKey` is the
/// wrong-password signal), load every row, and decrypt each secret into a
/// `SecureString`.
fn try_unlock(state: &Rc<RefCell<AppState>>, master: &SecureString) {
    let mut s = state.borrow_mut();
    let key = master.expose(|m| derive_key(m.as_bytes(), &s.salt));

    let store = match VaultStorage::open(&s.paths.db, &key) {
        Ok(store) => store,
        Err(storage::StorageError::BadKey) => {
            s.state = VaultState::Error("wrong master password".into());
            return;
        }
        Err(e) => {
            s.state = VaultState::Error(format!("{e}"));
            return;
        }
    };
    let stored = match store.load_entries() {
        Ok(v) => v,
        Err(e) => {
            s.state = VaultState::Error(format!("{e}"));
            return;
        }
    };

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let mut unlocked = Vec::with_capacity(stored.len());
    for e in stored {
        match cipher.decrypt(Nonce::from_slice(&e.nonce), e.ciphertext.as_ref()) {
            Ok(pt) => {
                let mut pt = Zeroizing::new(pt);
                let password = match std::str::from_utf8(&pt) {
                    Ok(txt) => SecureString::new(txt),
                    Err(_) => {
                        s.state = VaultState::Error("corrupt entry".into());
                        return;
                    }
                };
                pt.as_mut_slice().fill(0);
                unlocked.push(UnlockedEntry {
                    site: e.site,
                    username: e.username,
                    password,
                });
            }
            Err(_) => {
                s.state = VaultState::Error("corrupt entry (auth failed)".into());
                return;
            }
        }
    }
    s.state = VaultState::Unlocked(unlocked);
}

/// Lock screen — master password prompt. On unlock, hands off to the vault
/// screen; the `replace_screen` tears down this `SecureInput`, so the typed
/// password zeroizes as part of the transition.
fn build_lock_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    // Re-lock: drop any decrypted secrets before showing the prompt again.
    state.borrow_mut().state = VaultState::Locked;

    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .gap(12.0)
            .background(BG),
    );

    tree.add_child(
        root,
        TextWidget::new("shroud \u{2014} Vault")
            .font_size(24.0)
            .color(HEADING),
    );
    tree.add_child(
        root,
        TextWidget::new(format!("Master password (hint: {DEMO_PASSWORD})")).color(MUTED),
    );

    let unlock_state = Rc::clone(&state);
    tree.add_child(
        root,
        shroud::widgets::SecureInput::new()
            .placeholder("Enter master password, press Enter to unlock")
            .on_submit(move |master, ctx| {
                try_unlock(&unlock_state, master);
                if matches!(unlock_state.borrow().state, VaultState::Unlocked(_)) {
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
            VaultState::Error(e) => format!("Locked \u{2014} {e}"),
            VaultState::Unlocked(_) => "Unlocked \u{2014} opening vault\u{2026}".into(),
        })
        .color(MUTED),
    );
}

/// Vault screen — the entry list, a Lock button, and a clipboard-countdown
/// status line.
fn build_vault_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(20.0)
            .gap(12.0)
            .background(BG),
    );

    let header = tree.add_child(root, Container::row().width_full().gap(12.0));
    tree.add_child(
        header,
        TextWidget::new("shroud \u{2014} Vault")
            .font_size(22.0)
            .color(HEADING),
    );
    let lock_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::new("Lock").on_click(move |ctx| {
            lock_state.borrow_mut().state = VaultState::Locked;
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
                    let tail = remaining
                        .map(|d| {
                            format!(
                                " \u{2014} clipboard clears in {}s",
                                d.as_secs_f64().ceil() as u64
                            )
                        })
                        .unwrap_or_default();
                    format!("{} entries{tail}", v.len())
                }
                _ => "Locking\u{2026}".into(),
            }
        })
        .color(MUTED),
    );

    // The list lives inside a `ScrollView` either way; only *which* rows exist
    // differs. Default: a `VirtualList` materializing just the visible window.
    // `VAULT_PLAIN=1`: the old O(n) path (every row at once) — the A/B baseline.
    let list = tree.add_child(
        root,
        ScrollView::new().width_full().grow(1.0).background(PANEL),
    );
    if std::env::var_os("VAULT_PLAIN").is_some() {
        build_plain_rows(tree, list, Rc::clone(&state));
    } else {
        build_virtual_list(tree, list, Rc::clone(&state));
    }
}

/// Add one row for entry `idx` under `parent`. Shared by both list paths so the
/// A/B compares only *how many* rows exist, not how they look.
fn build_row(tree: &mut WidgetTree, parent: usize, state: &Rc<RefCell<AppState>>, idx: usize) {
    let row = tree.add_child(
        parent,
        Container::row()
            .width_full()
            .height(ROW_H)
            .gap(12.0)
            .padding(8.0)
            .background(ROW),
    );

    let label = {
        let s = state.borrow();
        match &s.state {
            VaultState::Unlocked(v) => match v.get(idx) {
                Some(e) => format!("{}  \u{00b7}  {}", e.site, e.username),
                None => String::new(),
            },
            _ => String::new(),
        }
    };
    tree.add_child(row, TextWidget::new(label));

    // Growing spacer pushes the Copy button to the row's right edge.
    tree.add_child(row, Container::row().grow(1.0));

    let copy_state = Rc::clone(state);
    tree.add_child(
        row,
        Button::new("Copy").on_click(move |_ctx| {
            let mut s = copy_state.borrow_mut();
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

/// Number of entries currently unlocked (0 while locked).
fn entry_count(state: &Rc<RefCell<AppState>>) -> usize {
    match &state.borrow().state {
        VaultState::Unlocked(v) => v.len(),
        _ => 0,
    }
}

/// Virtualized list — only the rows in (or near) the viewport are ever
/// materialized, regardless of [`SEED_COUNT`].
fn build_virtual_list(tree: &mut WidgetTree, sv_parent: usize, state: Rc<RefCell<AppState>>) {
    let count_state = Rc::clone(&state);
    VirtualList::new(ROW_H)
        .items(move || entry_count(&count_state))
        .on_row(move |tree, parent, idx| build_row(tree, parent, &state, idx))
        .build(tree, sv_parent);
}

/// Plain list — the O(n) baseline: every entry's row exists at once.
fn build_plain_rows(tree: &mut WidgetTree, parent: usize, state: Rc<RefCell<AppState>>) {
    for idx in 0..entry_count(&state) {
        build_row(tree, parent, &state, idx);
    }
}

fn main() {
    let paths = VaultPaths::default_for_app().expect("resolve a config dir for the vault");
    let salt = ensure_vault(&paths);

    App::new()
        .title("shroud \u{2014} Vault")
        .size(680, 620)
        // Kept off during the build so layout can be screenshotted; flip to
        // `true` for the shipping-secret posture (blacks out OS screen capture).
        .capture_prevention(false)
        .run(move |scope| {
            let state = Rc::new(RefCell::new(AppState {
                paths,
                salt,
                state: VaultState::Locked,
                clipboard: SecureClipboard::new(),
            }));

            // Advance the clipboard auto-clear timer on the UI thread.
            let tick_state = Rc::clone(&state);
            scope.on_frame(move |_ctx| {
                tick_state.borrow_mut().clipboard.tick();
            });

            // Ctrl+L — lock from anywhere.
            let lock_state = Rc::clone(&state);
            scope.on_shortcut(Shortcut::ctrl('l'), move |ctx| {
                let was_unlocked = {
                    let mut s = lock_state.borrow_mut();
                    let was = matches!(s.state, VaultState::Unlocked(_));
                    if was {
                        s.state = VaultState::Locked;
                    }
                    was
                };
                if was_unlocked {
                    let next = Rc::clone(&lock_state);
                    ctx.event_ctx
                        .replace_screen(move |tree| build_lock_screen(tree, next));
                }
            });

            let mut tree = WidgetTree::new();
            build_lock_screen(&mut tree, state);
            tree
        });
}
