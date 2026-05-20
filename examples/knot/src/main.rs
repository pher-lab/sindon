//! Knot M1 — multi-note in-memory port of the Knot v0.7.0 app onto shroud.
//!
//! Promoted from `examples/knot_spike`. Scope vs the spike:
//!   * spike: lock → 1 fixed note → unlock cycle, in-memory single ciphertext.
//!   * M1: lock → vault with N notes → sidebar + editor + add/delete +
//!     live title/body editing, all still in-memory (SQLCipher is M2).
//!
//! The crypto path is exercised on every save: each note's payload
//! (`title | body`) is re-encrypted under the resident master key. Locking
//! drops both the plaintext `Vec<Note>` and the `MasterKey`, leaving only
//! the encrypted vault in memory until the next unlock.

mod crypto;
mod editor;
mod lock_screen;
mod sidebar;
mod state;
mod vault_screen;

use std::cell::RefCell;
use std::rc::Rc;

use shroud::app::App;
use shroud::widgets::tree::WidgetTree;

use crate::crypto::{derive_key, random_salt};
use crate::state::{AppState, Note, NoteId};

pub const DEMO_PASSWORD: &str = "knot-demo-2026";

const DEMO_NOTES: &[(&str, &str)] = &[
    (
        "Welcome to Knot",
        "Knot is a privacy-first encrypted notes app. This M1 build is the\n\
         first end-to-end port onto shroud — the framework's own dogfood.\n\
         \n\
         Try editing this note; the body will be re-encrypted in memory on\n\
         every save trigger, so the cipher path is exercised on every\n\
         change. Lock to drop the plaintext key.",
    ),
    (
        "Roadmap",
        "M1 (this build):\n\
         - In-memory vault, plain-text editor, sidebar with add / delete\n\
         M2:\n\
         - SQLCipher persistence (rusqlite-sqlcipher), real save-on-disk\n\
         M3+:\n\
         - Setup screen, BIP39 recovery, settings persistence, themes\n\
         \n\
         The spike (examples/knot_spike) stays as the minimum-viable\n\
         reference. M1 lives here.",
    ),
    (
        "日本語テスト",
        "Knot は日本語のメモも普通に扱えます。\n\
         複数行・絵文字・長文の wrap も走るはず。\n\
         \n\
         このメモを編集して保存 → ロック → 解錠 すると、暗号化往復が\n\
         本当に走っていることが確認できる(中身が残っていれば成功)。",
    ),
];

/// Build the initial encrypted vault using the demo password. The salt is
/// generated fresh per launch (M1 has no on-disk persistence yet), but the
/// password is fixed so unlocking always uses `DEMO_PASSWORD`.
fn make_demo_vault() -> AppState {
    let salt = random_salt();
    let key = derive_key(DEMO_PASSWORD.as_bytes(), &salt);

    let initial_notes: Vec<Note> = DEMO_NOTES
        .iter()
        .enumerate()
        .map(|(i, (t, b))| Note {
            id: (i + 1) as NoteId,
            title: (*t).to_string(),
            body: (*b).to_string(),
        })
        .collect();
    let next_id = initial_notes.len() as NoteId + 1;

    let mut state = AppState::new(salt, Vec::new(), next_id);
    state.rewrite_vault(&key, &initial_notes);

    // Key goes out of scope → Zeroizing drops it. The vault stays encrypted
    // until the user enters the password on the lock screen and we re-derive.
    state
}

fn main() {
    App::new()
        .title("Knot \u{2014} M1")
        .size(1080, 720)
        .capture_prevention(true)
        .run(|_scope| {
            let state = Rc::new(RefCell::new(make_demo_vault()));

            let mut tree = WidgetTree::new();
            lock_screen::build(&mut tree, state);
            tree
        });
}
