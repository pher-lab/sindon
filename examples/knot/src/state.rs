//! In-memory state for Knot M1.
//!
//! No SQLCipher yet — notes live in a `Vec<EncryptedNote>` while locked and
//! a `Vec<Note>` (= plaintext) while unlocked. On lock, the plaintext vec is
//! dropped (zeroize-on-drop on the body strings would be ideal; the spike
//! relies on `String::drop` + the upcoming `replace_screen` to deallocate).

use crate::crypto::{KEY_SIZE, MasterKey, NONCE_SIZE, SALT_SIZE, open, seal};

pub type NoteId = u32;

/// On-disk shape (modulo: M1 has no disk yet). The ciphertext is produced
/// by `XChaCha20Poly1305::encrypt` so it already contains the auth tag.
pub struct EncryptedNote {
    pub id: NoteId,
    pub nonce: [u8; NONCE_SIZE],
    pub ciphertext: Vec<u8>,
}

/// Decrypted note as shown in the editor. Title is held plaintext alongside
/// the body for editing convenience — when we save, the whole `{title, body}`
/// payload is serialized and encrypted as one blob.
#[derive(Clone)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub body: String,
}

pub enum Phase {
    /// No vault loaded. `salt` is fixed for the demo so that lock → unlock
    /// of the same in-memory vault works across cycles.
    Locked { error: Option<String> },
    /// Vault decrypted and resident.
    Unlocked {
        key: MasterKey,
        notes: Vec<Note>,
        selected: Option<NoteId>,
    },
}

pub struct AppState {
    pub salt: [u8; SALT_SIZE],
    /// The encrypted store. Lives across lock cycles.
    pub vault: Vec<EncryptedNote>,
    pub next_id: NoteId,
    pub phase: Phase,
}

impl AppState {
    pub fn new(salt: [u8; SALT_SIZE], vault: Vec<EncryptedNote>, next_id: NoteId) -> Self {
        Self {
            salt,
            vault,
            next_id,
            phase: Phase::Locked { error: None },
        }
    }

    /// Try to decrypt every note in the vault with `key`. Returns the list
    /// or `None` if any single entry fails to authenticate (a wrong-password
    /// situation is detected here).
    pub fn try_decrypt_all(&self, key: &MasterKey) -> Option<Vec<Note>> {
        let mut out = Vec::with_capacity(self.vault.len());
        for enc in &self.vault {
            let pt = open(key, &enc.nonce, &enc.ciphertext)?;
            let (title, body) = split_payload(&pt)?;
            out.push(Note {
                id: enc.id,
                title,
                body,
            });
        }
        Some(out)
    }

    /// Re-encrypt the resident notes into the vault, then drop both the
    /// plaintext notes and the master key by transitioning to
    /// `Phase::Locked`. No-op if already locked.
    ///
    /// `Zeroizing` on the key handles its zeroize-on-drop; the body
    /// `String`s drop normally (a future phase can swap them for
    /// `SecureString` or similar if we want zeroize on edit too).
    pub fn lock_and_seal(&mut self) {
        let old = std::mem::replace(&mut self.phase, Phase::Locked { error: None });
        if let Phase::Unlocked { key, notes, .. } = old {
            self.rewrite_vault(&key, &notes);
            // `key` and `notes` drop here.
        }
    }

    /// Re-encrypt and rewrite the vault from the current `notes` slice.
    /// Called on save / new note / delete.
    pub fn rewrite_vault(&mut self, key: &MasterKey, notes: &[Note]) {
        let mut new_vault = Vec::with_capacity(notes.len());
        for note in notes {
            let payload = join_payload(&note.title, &note.body);
            let (nonce, ct) = seal(key, &payload);
            new_vault.push(EncryptedNote {
                id: note.id,
                nonce,
                ciphertext: ct,
            });
        }
        self.vault = new_vault;
    }
}

/// Payload encoding: 4-byte big-endian title length + title bytes + body bytes.
/// Plenty for M1 — switch to bincode/postcard when we add tags/timestamps.
fn join_payload(title: &str, body: &str) -> Vec<u8> {
    let tb = title.as_bytes();
    let bb = body.as_bytes();
    let mut out = Vec::with_capacity(4 + tb.len() + bb.len());
    let len = u32::try_from(tb.len()).expect("title under 4GB");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(tb);
    out.extend_from_slice(bb);
    out
}

fn split_payload(bytes: &[u8]) -> Option<(String, String)> {
    if bytes.len() < 4 {
        return None;
    }
    let title_len = u32::from_be_bytes(bytes[..4].try_into().ok()?) as usize;
    if bytes.len() < 4 + title_len {
        return None;
    }
    let title = String::from_utf8(bytes[4..4 + title_len].to_vec()).ok()?;
    let body = String::from_utf8(bytes[4 + title_len..].to_vec()).ok()?;
    Some((title, body))
}

const _ASSERT_KEY_SIZE: usize = KEY_SIZE;
