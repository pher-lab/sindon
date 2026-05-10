//! knot_spike — minimum viable port of the Knot notes app onto shroud.
//!
//! Goal: verify shroud can host the smallest end-to-end Knot flow:
//! lock screen → derive key → decrypt → display 1 read-only note.
//! Storage (SQLCipher) and editing are intentionally out of scope; a single
//! plaintext note is encrypted in memory at startup so the spike exercises
//! the same crypto path as the real app without dragging in `rusqlite-sqlcipher`.
//!
//! Crypto matches Knot v0.7.0:
//! - Argon2id with 64MB / 3 iter / 4 lanes (heavyweight — derivation will
//!   freeze the UI for ~1-2 s on a typical desktop. That freeze is one of
//!   the gaps the spike is meant to surface).
//! - XChaCha20-Poly1305 with [version | nonce(24) | ciphertext+tag] format.
//!
//! Hardcoded demo master password: see `DEMO_PASSWORD`.

use std::cell::RefCell;
use std::rc::Rc;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use zeroize::Zeroizing;

use shroud::app::App;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, ScrollView, SecureInput, TextWidget};

const DEMO_PASSWORD: &str = "knot-demo-2026";
const DEMO_NOTE_TITLE: &str = "Welcome to Knot";
const DEMO_NOTE_BODY: &str = "\
This is a demo note encrypted at startup with the demo password.

Notes are stored encrypted with XChaCha20-Poly1305 in production. The whole
database file is additionally encrypted via SQLCipher; this spike skips that
outer layer.

Master password is run through Argon2id (64MB, 3 iterations, 4 lanes), the
RFC 9106 high-memory profile. Decrypted note content lives only in memory
and disappears when the app is locked.

This paragraph is long enough that the renderer should wrap it across
multiple lines, so the spike can verify wrapping and scrolling actually work.
Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam,
quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo
consequat.

Edge cases worth eyeballing:
- Multi-byte unicode: 日本語のテストです。これも正しく描画されるはず。
- Emoji glyphs: locking icon + key (skipped here to avoid font fallback noise).
- A long word with no break opportunities follows on the next line.
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA

End of demo note.
";

const SALT_SIZE: usize = 32;
const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 24;

/// In-memory ciphertext store — stand-in for what would otherwise come out
/// of SQLCipher. Title is kept plaintext for spike simplicity; in real Knot
/// the title is encrypted too.
struct EncryptedNote {
    title: String,
    nonce: [u8; NONCE_SIZE],
    ciphertext: Vec<u8>,
}

/// Decrypted note as displayed on screen. Lives only inside `AppPhase::Unlocked`.
struct NoteContent {
    title: String,
    body: String,
}

enum AppPhase {
    Locked,
    Unlocked { note: NoteContent },
    Error(String),
}

struct AppState {
    salt: [u8; SALT_SIZE],
    encrypted: EncryptedNote,
    phase: AppPhase,
    /// Resident master key while unlocked. The spike only needs it to track
    /// "we are unlocked" — a real Knot port would re-use it for editing.
    key: Option<Zeroizing<[u8; KEY_SIZE]>>,
}

fn derive_key(password: &[u8], salt: &[u8]) -> Zeroizing<[u8; KEY_SIZE]> {
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        // Knot v0.7.0 production parameters.
        Params::new(64 * 1024, 3, 4, Some(KEY_SIZE)).expect("valid argon2 params"),
    );
    let mut key = Zeroizing::new([0u8; KEY_SIZE]);
    argon2
        .hash_password_into(password, salt, key.as_mut())
        .expect("argon2 kdf");
    key
}

fn make_demo_vault() -> ([u8; SALT_SIZE], EncryptedNote) {
    let mut salt = [0u8; SALT_SIZE];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(DEMO_PASSWORD.as_bytes(), &salt);
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("32-byte key");

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, DEMO_NOTE_BODY.as_bytes())
        .expect("encrypt demo note");

    (
        salt,
        EncryptedNote {
            title: DEMO_NOTE_TITLE.to_string(),
            nonce: nonce_bytes,
            ciphertext,
        },
    )
}

/// Derive the master key, attempt decryption. AEAD failure → `Error`. Success
/// stores the decrypted body in `AppPhase::Unlocked` and the key in
/// `AppState::key`.
fn try_unlock(state: &Rc<RefCell<AppState>>, master: &SecureString) {
    let mut s = state.borrow_mut();
    let key = master.expose(|m| derive_key(m.as_bytes(), &s.salt));
    let cipher = match XChaCha20Poly1305::new_from_slice(key.as_ref()) {
        Ok(c) => c,
        Err(_) => {
            s.phase = AppPhase::Error("cipher init failed".into());
            return;
        }
    };
    let nonce = XNonce::from_slice(&s.encrypted.nonce);
    match cipher.decrypt(nonce, s.encrypted.ciphertext.as_ref()) {
        Ok(pt) => {
            let body = match String::from_utf8(pt) {
                Ok(s) => s,
                Err(_) => {
                    s.phase = AppPhase::Error("corrupt note".into());
                    s.key = None;
                    return;
                }
            };
            s.phase = AppPhase::Unlocked {
                note: NoteContent {
                    title: s.encrypted.title.clone(),
                    body,
                },
            };
            s.key = Some(key);
        }
        Err(_) => {
            s.phase = AppPhase::Error("wrong master password".into());
            s.key = None;
        }
    }
}

fn build_lock_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    // Reset transient state on (re)entry, but preserve `Error` so the user
    // sees why they bounced back from a failed unlock.
    {
        let mut s = state.borrow_mut();
        s.key = None;
        if matches!(s.phase, AppPhase::Unlocked { .. }) {
            s.phase = AppPhase::Locked;
        }
    }

    // Centered card layout. Outer container vertically centers via
    // `justify_center` (main axis only — leaves cross axis at Stretch so
    // children are not collapsed to min-content). The inner card declares
    // `width_full().max_width(448)` so it stays readable on wide windows
    // and fluid on narrow ones, mirroring Tailwind's `w-full max-w-md`.
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .justify_center()
            .align_center(),
    );

    let card = tree.add_child(
        root,
        Container::column()
            .width_full()
            .max_width(448.0)
            .padding(32.0)
            .gap(16.0),
    );

    tree.add_child(card, TextWidget::new("Knot").font_size(40.0));
    tree.add_child(card, TextWidget::new("A knot only you can untie."));
    tree.add_child(
        card,
        TextWidget::new(format!("Master password (hint: {}):", DEMO_PASSWORD)),
    );

    let unlock_state = Rc::clone(&state);
    let input_idx = tree.add_child(
        card,
        SecureInput::new()
            .placeholder("Enter master password, press Enter to unlock")
            .on_submit(move |master, ctx| {
                try_unlock(&unlock_state, master);
                let unlocked = matches!(unlock_state.borrow().phase, AppPhase::Unlocked { .. });
                if unlocked {
                    let next = Rc::clone(&unlock_state);
                    ctx.replace_screen(move |tree| build_note_screen(tree, next));
                }
            }),
    );
    // Auto-focus the master-password field so the user can type immediately
    // — Knot React parity (`<input autoFocus>`) plus the same parity after
    // bouncing back from the note screen via Lock. The build path has no
    // EventContext, so the focus has to be queued and applied by the event
    // loop on the first redraw.
    tree.focus_initially(input_idx);

    let status_state = Rc::clone(&state);
    tree.add_child(
        card,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            AppPhase::Locked => "Locked.".into(),
            AppPhase::Error(e) => format!("Locked \u{2014} {}", e),
            // The handler transitions away on success; only on screen for a
            // single frame before the screen swap drains.
            AppPhase::Unlocked { .. } => "Unlocked \u{2014} opening note\u{2026}".into(),
        })
        .color(shroud::core::Color::rgb(0.7, 0.7, 0.75)),
    );
}

fn build_note_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .gap(12.0),
    );

    // Header: title + Lock button. `align_center` sizes each child to its
    // own height and centers them on the cross (vertical) axis — without it,
    // the row's default `Stretch` would extend the button to the title's
    // 28 pt height and the label would visually drift below the title baseline.
    let header = tree.add_child(root, Container::row().gap(12.0).align_center());

    let title_state = Rc::clone(&state);
    tree.add_child(
        header,
        TextWidget::reactive(move || match &title_state.borrow().phase {
            AppPhase::Unlocked { note } => note.title.clone(),
            _ => String::new(),
        })
        .font_size(28.0),
    );

    let lock_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::new("Lock").on_click(move |ctx| {
            // Drop decrypted body + key first to keep the
            // "secret-in-memory after lock-pressed" window minimal.
            {
                let mut s = lock_state.borrow_mut();
                s.phase = AppPhase::Locked;
                s.key = None;
            }
            let next = Rc::clone(&lock_state);
            ctx.replace_screen(move |tree| build_lock_screen(tree, next));
        }),
    );

    // Body — wrapped inside a ScrollView. ScrollView (Phase 11) requires an
    // explicit content_height; we approximate from line count. This is
    // gap-worthy: a real port wants "auto from laid-out children".
    let body_text = match &state.borrow().phase {
        AppPhase::Unlocked { note } => note.body.clone(),
        _ => String::new(),
    };
    let approx_lines = body_text.lines().count().max(1) as f32;
    // Generous slack so wrapped lines don't overflow the declared content
    // height. Line height ~22 + buffer for wraps.
    let approx_height = approx_lines * 24.0 + 240.0;

    let scroll = tree.add_child(
        root,
        ScrollView::new()
            .width_full()
            .height(480.0)
            .content_height(approx_height),
    );

    tree.add_child(scroll, TextWidget::new(body_text));
}

fn main() {
    App::new()
        .title("Knot \u{2014} shroud spike")
        .size(720, 600)
        .capture_prevention(true)
        .run(|_scope| {
            let (salt, encrypted) = make_demo_vault();

            let state = Rc::new(RefCell::new(AppState {
                salt,
                encrypted,
                phase: AppPhase::Locked,
                key: None,
            }));

            let mut tree = WidgetTree::new();
            build_lock_screen(&mut tree, state);
            tree
        });
}
