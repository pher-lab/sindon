//! Knot M2 state.
//!
//! Holds a `Phase::Unlocked` carrying the live `VaultStorage` connection
//! alongside the resident plaintext notes, master key, and a `dirty`
//! set of note ids that the auto-save tick will flush back to disk.
//! Locking drops the entire `Unlocked` variant — connection, key, notes,
//! and dirty set all go in one move, which is what makes "lock" actually
//! release plaintext / re-seal the on-disk DB.

use std::collections::HashSet;

use crate::crypto::{KEY_SIZE, MasterKey, NONCE_SIZE, SALT_SIZE, open, seal};
use crate::storage::{StorageError, VaultStorage};

pub type NoteId = u32;

/// On-disk shape of a single note row. The ciphertext already contains
/// the XChaCha20-Poly1305 auth tag; the SQLCipher layer adds page-level
/// encryption *on top* of this when these bytes are stored.
pub struct EncryptedNote {
    pub id: NoteId,
    pub nonce: [u8; NONCE_SIZE],
    pub ciphertext: Vec<u8>,
}

/// Decrypted note as shown in the editor. Title is held plaintext
/// alongside the body for editing convenience — when saving, the whole
/// `{title, body}` payload is serialized then encrypted as one blob.
#[derive(Clone)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub body: String,
}

pub enum Phase {
    /// No vault loaded. `error` carries the most recent unlock-attempt
    /// failure so the lock screen can echo it back as red status text.
    Locked { error: Option<String> },
    /// Vault decrypted and resident. The `VaultStorage` connection stays
    /// open for the entire unlocked session — auto-save uses it on
    /// every flush tick, and locking drops it (closing the conn +
    /// re-sealing the file) by replacing this whole variant.
    Unlocked {
        key: MasterKey,
        notes: Vec<Note>,
        selected: Option<NoteId>,
        storage: VaultStorage,
        /// Ids of notes mutated since the last successful flush. The
        /// auto-save tick drains this; a flush failure leaves the set
        /// alone so the next tick retries instead of silently losing
        /// the edit.
        dirty: HashSet<NoteId>,
    },
}

pub struct AppState {
    pub salt: [u8; SALT_SIZE],
    pub next_id: NoteId,
    pub phase: Phase,
}

impl AppState {
    /// Construct a Locked state. The vault contents on disk are not
    /// touched here — they're loaded once the user types their
    /// password and the lock screen calls `unlock_with`.
    pub fn new_locked(salt: [u8; SALT_SIZE], next_id: NoteId) -> Self {
        Self {
            salt,
            next_id,
            phase: Phase::Locked { error: None },
        }
    }

    /// Transition into `Unlocked` with the given decrypted state.
    /// Called by both the lock screen (after a successful password
    /// attempt) and the first-launch seed path in `main.rs`.
    pub fn become_unlocked(
        &mut self,
        key: MasterKey,
        notes: Vec<Note>,
        storage: VaultStorage,
    ) {
        // Pick the highest existing id + 1 as the next allocation point
        // so a relaunch doesn't recycle ids that crashed before saving.
        let max_id = notes.iter().map(|n| n.id).max().unwrap_or(0);
        self.next_id = max_id.saturating_add(1);
        let selected = notes.first().map(|n| n.id);
        self.phase = Phase::Unlocked {
            key,
            notes,
            selected,
            storage,
            dirty: HashSet::new(),
        };
    }

    /// Mark a note as needing to be written back on the next auto-save
    /// tick. No-op when not unlocked or when the id isn't in the
    /// resident vec (defends against races where the editor's last
    /// callback fires after a delete).
    pub fn mark_dirty(&mut self, id: NoteId) {
        if let Phase::Unlocked { notes, dirty, .. } = &mut self.phase {
            if notes.iter().any(|n| n.id == id) {
                dirty.insert(id);
            }
        }
    }

    /// Convenience: mark the currently selected note dirty. Used by
    /// the editor's `on_change` callbacks, which never know an explicit
    /// id — they only know "the field the user is typing in."
    pub fn mark_selected_dirty(&mut self) {
        if let Phase::Unlocked {
            selected: Some(id),
            notes,
            dirty,
            ..
        } = &mut self.phase
        {
            if notes.iter().any(|n| n.id == *id) {
                dirty.insert(*id);
            }
        }
    }

    /// Persist every dirty note via `storage.save_note`. Clears the
    /// dirty set on success; on the first error, the offending id is
    /// preserved (along with every later one) so the next tick retries.
    /// Errors are returned to the caller so the on_frame hook can log
    /// them — the UI deliberately does not surface flush failures to
    /// the user yet (no banner / toast widget in scope for M2).
    pub fn flush_dirty(&mut self) -> Result<usize, StorageError> {
        let Phase::Unlocked {
            key,
            notes,
            storage,
            dirty,
            ..
        } = &mut self.phase
        else {
            return Ok(0);
        };
        if dirty.is_empty() {
            return Ok(0);
        }
        // Drain in id order so retries (if a later note fails to save)
        // make consistent progress instead of churning across the set.
        let mut ids: Vec<NoteId> = dirty.iter().copied().collect();
        ids.sort_unstable();
        let mut flushed = 0usize;
        for id in ids {
            let Some(note) = notes.iter().find(|n| n.id == id) else {
                // Note got deleted between mark_dirty and flush — drop
                // it from the set silently. The deletion path itself
                // already removed the row from storage.
                dirty.remove(&id);
                continue;
            };
            let payload = join_payload(&note.title, &note.body);
            let (nonce, ciphertext) = seal(key, &payload);
            let row = EncryptedNote {
                id: note.id,
                nonce,
                ciphertext,
            };
            storage.save_note(&row)?;
            dirty.remove(&id);
            flushed += 1;
        }
        Ok(flushed)
    }

    /// Re-encrypt and persist the full set of notes in one transaction.
    /// Used by add / delete (the row layout in `notes` changed) and by
    /// `lock_and_seal` (belt-and-suspenders sweep before dropping the
    /// connection). Clears `dirty` on success.
    pub fn rewrite_vault_to_storage(&mut self) -> Result<(), StorageError> {
        let Phase::Unlocked {
            key,
            notes,
            storage,
            dirty,
            ..
        } = &mut self.phase
        else {
            return Ok(());
        };
        let rows: Vec<EncryptedNote> = notes
            .iter()
            .map(|note| {
                let payload = join_payload(&note.title, &note.body);
                let (nonce, ciphertext) = seal(key, &payload);
                EncryptedNote {
                    id: note.id,
                    nonce,
                    ciphertext,
                }
            })
            .collect();
        storage.save_all_notes(&rows)?;
        dirty.clear();
        Ok(())
    }

    /// Remove `id` from both the resident notes vec and the on-disk
    /// store. The sidebar's ✕ button calls this so a delete is durable
    /// even without waiting for the auto-save tick.
    ///
    /// Errors propagate to the caller. The in-memory removal still
    /// happens on storage failure (defensive: don't leave the user
    /// staring at a row they thought they deleted).
    pub fn delete_note_persisted(&mut self, id: NoteId) -> Result<bool, StorageError> {
        let Phase::Unlocked {
            notes,
            selected,
            storage,
            dirty,
            ..
        } = &mut self.phase
        else {
            return Ok(false);
        };
        let before = notes.len();
        notes.retain(|n| n.id != id);
        let removed = notes.len() < before;
        dirty.remove(&id);
        if *selected == Some(id) {
            *selected = notes.first().map(|n| n.id);
        }
        if removed {
            storage.delete_note(id)?;
        }
        Ok(removed)
    }

    /// Re-encrypt the resident notes and transition to `Locked`.
    /// Failures are logged (`log::error!`) but do not block the lock —
    /// the user asked to lock, and stalling on a write error would
    /// leave the master key resident, which is the worse outcome for
    /// a "secret-aware" framework.
    pub fn lock_and_seal(&mut self) {
        if matches!(self.phase, Phase::Locked { .. }) {
            return;
        }
        if let Err(e) = self.rewrite_vault_to_storage() {
            // No logging framework wired into knot — stderr is the
            // pragmatic channel for "something went wrong on disk."
            // M3 candidate: surface to a status banner in the lock
            // screen so the user knows their last edits didn't land.
            eprintln!(
                "knot: failed to flush vault on lock — unsaved edits lost: {}",
                e
            );
        }
        // Replace the variant; key, storage (closing conn), notes,
        // dirty all drop here.
        self.phase = Phase::Locked { error: None };
    }
}

/// Decrypt every entry under `key`. Returns `None` if any single row
/// fails to authenticate (= wrong password). Pulled out of `AppState`
/// because the lock-screen flow needs it before any `Unlocked` state
/// exists.
pub fn decrypt_all(key: &MasterKey, vault: &[EncryptedNote]) -> Option<Vec<Note>> {
    let mut out = Vec::with_capacity(vault.len());
    for enc in vault {
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

/// Payload encoding: 4-byte big-endian title length + title bytes + body bytes.
/// Plenty for M2 — switch to bincode/postcard when we add tags / timestamps.
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
