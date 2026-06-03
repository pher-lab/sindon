//! Knot state.
//!
//! Holds a `Phase::Unlocked` carrying the live `VaultStorage` connection
//! alongside the resident plaintext notes, the DEK (data-encryption key
//! that SQLCipher and the per-note `seal` use), and a `dirty` set of note
//! ids that the auto-save tick will flush back to disk. Locking drops the
//! entire `Unlocked` variant — connection, DEK, notes, and dirty set all
//! go in one move, which is what makes "lock" actually release plaintext /
//! re-seal the on-disk DB.

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
/// `{title, tags, body}` payload is serialized then encrypted as one blob.
#[derive(Clone)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub body: String,
    /// Free-form labels, normalized to lowercase + trimmed and kept unique
    /// in insertion order (see [`normalize_tag`]). Persisted inside the
    /// encrypted payload alongside title/body — never as plaintext metadata.
    pub tags: Vec<String>,
}

// The `Unlocked` variant is much larger than the others — it carries the
// live SQLCipher connection, the DEK, the resident notes and the dirty/filter
// sets, i.e. the whole decrypted session — while `Setup` / `Locked` /
// `Recovery` hold only an `Option<String>`. That size gap trips
// `large_enum_variant`, but there is exactly one `Phase` value alive at a time
// (it's the app's state machine, never stored in a collection), so the lint's
// memory-waste rationale doesn't apply; boxing the live connection would only
// add an allocation + deref on the hot auto-save path for no real gain.
#[allow(clippy::large_enum_variant)]
pub enum Phase {
    /// First launch — no vault on disk yet. The setup screen collects a
    /// master password (and a confirmation) before any salt or DB
    /// exists. `error` carries the most recent setup-attempt failure
    /// (too short, mismatch, write error) for the status line to echo.
    Setup { error: Option<String> },
    /// No vault loaded. `error` carries the most recent unlock-attempt
    /// failure so the lock screen can echo it back as red status text.
    Locked { error: Option<String> },
    /// Forgot-password flow: the user supplies their BIP39 recovery key
    /// plus a new password. `error` carries the most recent recovery
    /// failure (bad mnemonic, mismatch, no recovery wrapping on disk).
    Recovery { error: Option<String> },
    /// Vault decrypted and resident. The `VaultStorage` connection stays
    /// open for the entire unlocked session — auto-save uses it on
    /// every flush tick, and locking drops it (closing the conn +
    /// re-sealing the file) by replacing this whole variant.
    Unlocked {
        /// Data-encryption key: keys SQLCipher and every note `seal`/`open`.
        /// Generated once at setup, wrapped on disk under the password and
        /// recovery KEKs — never derived from the password directly.
        dek: MasterKey,
        notes: Vec<Note>,
        selected: Option<NoteId>,
        storage: VaultStorage,
        /// Ids of notes mutated since the last successful flush. The
        /// auto-save tick drains this; a flush failure leaves the set
        /// alone so the next tick retries instead of silently losing
        /// the edit.
        dirty: HashSet<NoteId>,
        /// Tags the sidebar is currently filtering the note list by
        /// (intersection — a note shows only if it carries *every* tag here;
        /// empty = show all). Pure session UI state that lives and dies with
        /// the unlocked vault, exactly like `selected`; it is never persisted.
        filter_tags: Vec<String>,
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

    /// Construct a first-launch Setup state. The salt is a placeholder —
    /// it's generated for real when the user picks a password and
    /// [`complete_setup`](Self::complete_setup) overwrites it. `next_id`
    /// starts at 1 since the vault is empty until the user adds a note.
    pub fn new_setup() -> Self {
        Self {
            salt: [0u8; SALT_SIZE],
            next_id: 1,
            phase: Phase::Setup { error: None },
        }
    }

    /// Transition into `Unlocked` with the given decrypted state. `dek` is
    /// the data-encryption key (unwrapped from `dek.enc` / `recovery.enc`),
    /// not the password. Called by the lock screen, the recovery screen,
    /// and the first-launch setup path.
    pub fn become_unlocked(&mut self, dek: MasterKey, notes: Vec<Note>, storage: VaultStorage) {
        // Pick the highest existing id + 1 as the next allocation point
        // so a relaunch doesn't recycle ids that crashed before saving.
        let max_id = notes.iter().map(|n| n.id).max().unwrap_or(0);
        self.next_id = max_id.saturating_add(1);
        let selected = notes.first().map(|n| n.id);
        self.phase = Phase::Unlocked {
            dek,
            notes,
            selected,
            storage,
            dirty: HashSet::new(),
            filter_tags: Vec::new(),
        };
    }

    /// Finish first-launch setup: record the freshly-generated salt and
    /// transition straight into `Unlocked` with the (initially empty)
    /// note set. Distinct from [`become_unlocked`](Self::become_unlocked)
    /// only in that it also installs the real salt, which the placeholder
    /// from [`new_setup`](Self::new_setup) was standing in for.
    pub fn complete_setup(
        &mut self,
        salt: [u8; SALT_SIZE],
        dek: MasterKey,
        notes: Vec<Note>,
        storage: VaultStorage,
    ) {
        self.salt = salt;
        self.become_unlocked(dek, notes, storage);
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

    /// Tags of the currently selected note, in insertion order. Empty when
    /// nothing is selected or the app is locked. Used by the tag editor to
    /// (re)render the chip row.
    pub fn selected_tags(&self) -> Vec<String> {
        if let Phase::Unlocked {
            selected: Some(id),
            notes,
            ..
        } = &self.phase
        {
            if let Some(note) = notes.iter().find(|n| n.id == *id) {
                return note.tags.clone();
            }
        }
        Vec::new()
    }

    /// Every distinct tag across the whole vault, sorted. Feeds the tag
    /// editor's autocomplete suggestions — the union of what the user has
    /// used before so related notes converge on the same labels.
    pub fn all_tags(&self) -> Vec<String> {
        let Phase::Unlocked { notes, .. } = &self.phase else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for note in notes {
            for tag in &note.tags {
                if !out.contains(tag) {
                    out.push(tag.clone());
                }
            }
        }
        out.sort();
        out
    }

    /// Add a tag to the selected note. Normalizes (lowercase + trim) and
    /// no-ops on an empty result or a duplicate. Marks the note dirty on a
    /// real insertion so the auto-save tick persists it. Returns whether a
    /// tag was actually added.
    pub fn add_tag_to_selected(&mut self, raw: &str) -> bool {
        let tag = normalize_tag(raw);
        if tag.is_empty() {
            return false;
        }
        let added = if let Phase::Unlocked {
            selected: Some(id),
            notes,
            ..
        } = &mut self.phase
        {
            if let Some(note) = notes.iter_mut().find(|n| n.id == *id) {
                if note.tags.iter().any(|t| t == &tag) {
                    false
                } else {
                    note.tags.push(tag);
                    true
                }
            } else {
                false
            }
        } else {
            false
        };
        if added {
            self.mark_selected_dirty();
        }
        added
    }

    /// Remove a tag from the selected note (exact match against the already
    /// normalized stored form). Marks the note dirty when a tag was removed.
    pub fn remove_tag_from_selected(&mut self, tag: &str) -> bool {
        let removed = if let Phase::Unlocked {
            selected: Some(id),
            notes,
            ..
        } = &mut self.phase
        {
            if let Some(note) = notes.iter_mut().find(|n| n.id == *id) {
                let before = note.tags.len();
                note.tags.retain(|t| t != tag);
                note.tags.len() < before
            } else {
                false
            }
        } else {
            false
        };
        if removed {
            self.mark_selected_dirty();
        }
        removed
    }

    // ── Sidebar tag filter ─────────────────────────────────────────────
    //
    // Session-only UI state: which tags the note list is narrowed by. Lives
    // in `Unlocked` next to `selected`, so it drops on lock and is never
    // persisted. Semantics are intersection (AND) — see `note_matches_filter`.

    /// Whether any tag filter is active (the note list is being narrowed).
    pub fn is_filtering(&self) -> bool {
        matches!(&self.phase, Phase::Unlocked { filter_tags, .. } if !filter_tags.is_empty())
    }

    /// Whether `tag` (already in normalized form) is one of the active filter
    /// tags. Drives a filter chip's highlighted/selected styling.
    pub fn is_filter_active(&self, tag: &str) -> bool {
        matches!(&self.phase, Phase::Unlocked { filter_tags, .. } if filter_tags.iter().any(|t| t == tag))
    }

    /// Whether any note carries at least one tag — i.e. whether there is
    /// anything to filter by at all. Lets the sidebar hide the filter row
    /// entirely on a vault with no tags yet.
    pub fn has_any_tags(&self) -> bool {
        matches!(&self.phase, Phase::Unlocked { notes, .. } if notes.iter().any(|n| !n.tags.is_empty()))
    }

    /// Toggle a tag in the sidebar filter, normalizing it first so it
    /// compares against the (already normalized) stored note tags. Returns
    /// the new state (true = now filtering on it). No-op on an empty input or
    /// when locked.
    pub fn toggle_filter_tag(&mut self, raw: &str) -> bool {
        let tag = normalize_tag(raw);
        if tag.is_empty() {
            return false;
        }
        if let Phase::Unlocked { filter_tags, .. } = &mut self.phase {
            if let Some(pos) = filter_tags.iter().position(|t| t == &tag) {
                filter_tags.remove(pos);
                false
            } else {
                filter_tags.push(tag);
                true
            }
        } else {
            false
        }
    }

    /// Clear the sidebar filter, so every note shows again.
    pub fn clear_filter(&mut self) {
        if let Phase::Unlocked { filter_tags, .. } = &mut self.phase {
            filter_tags.clear();
        }
    }

    /// Drop filter tags that no longer exist anywhere in the vault — e.g. the
    /// last note carrying one was deleted, or had the tag removed in the
    /// editor. Without this the list could stay filtered on a tag with no chip
    /// left to toggle it off, stranding the user on an empty list.
    pub fn prune_filter(&mut self) {
        let existing = self.all_tags();
        if let Phase::Unlocked { filter_tags, .. } = &mut self.phase {
            filter_tags.retain(|t| existing.contains(t));
        }
    }

    /// Note ids to show in the sidebar given the active filter, in stored
    /// order. With no filter this is every note; otherwise only notes that
    /// carry *all* the active filter tags (see `note_matches_filter`). Empty
    /// when locked.
    pub fn filtered_note_ids(&self) -> Vec<NoteId> {
        let Phase::Unlocked {
            notes, filter_tags, ..
        } = &self.phase
        else {
            return Vec::new();
        };
        notes
            .iter()
            .filter(|n| note_matches_filter(&n.tags, filter_tags))
            .map(|n| n.id)
            .collect()
    }

    /// Total resident note count, ignoring the filter. Lets the sidebar tell
    /// "no notes yet" apart from "the filter hid them all". Zero when locked.
    pub fn note_count(&self) -> usize {
        match &self.phase {
            Phase::Unlocked { notes, .. } => notes.len(),
            _ => 0,
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
            dek,
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
            let payload = join_payload(&note.title, &note.body, &note.tags);
            let (nonce, ciphertext) = seal(dek, &payload);
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
            dek,
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
                let payload = join_payload(&note.title, &note.body, &note.tags);
                let (nonce, ciphertext) = seal(dek, &payload);
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

/// Decrypt every entry under `dek`. Returns `None` if any single row
/// fails to authenticate (= tampered DB, since the DEK is already proven
/// correct by SQLCipher opening). Pulled out of `AppState` because the
/// lock / recovery flows need it before any `Unlocked` state exists.
pub fn decrypt_all(dek: &MasterKey, vault: &[EncryptedNote]) -> Option<Vec<Note>> {
    let mut out = Vec::with_capacity(vault.len());
    for enc in vault {
        let pt = open(dek, &enc.nonce, &enc.ciphertext)?;
        let (title, body, tags) = split_payload(&pt)?;
        out.push(Note {
            id: enc.id,
            title,
            body,
            tags,
        });
    }
    Some(out)
}

/// Normalize a raw tag input to its stored form: trimmed and lowercased.
/// Tags compare and dedupe in this form so `Work`, ` work ` and `WORK`
/// collapse to one. An all-whitespace input normalizes to the empty string,
/// which callers treat as "no tag."
pub fn normalize_tag(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// True when `tags` satisfies the active `filter`. An empty filter matches
/// everything; otherwise every filter tag must be present (intersection /
/// AND — each tag added to the filter narrows the result further). Both sides
/// are already normalized (note tags by [`AppState::add_tag_to_selected`],
/// filter tags by [`AppState::toggle_filter_tag`]), so this is a plain
/// membership test. Flip the `all` to `any` here for OR/union semantics.
pub fn note_matches_filter(tags: &[String], filter: &[String]) -> bool {
    filter.iter().all(|f| tags.iter().any(|t| t == f))
}

/// First byte of a v1 payload. The legacy format begins with the
/// big-endian *high* byte of the title length, which is `0` for any
/// realistic title (< 16 MiB), so a leading `1` unambiguously flags the
/// newer tag-carrying layout. Old vaults written before tags existed stay
/// readable; new writes always use v1.
const PAYLOAD_V1: u8 = 1;

/// Payload encoding (v1):
///
/// ```text
/// [1]  version = PAYLOAD_V1
/// [4]  title length, big-endian
/// [N]  title bytes
/// [2]  tag count, big-endian
///   repeated: [2] tag length BE, [M] tag bytes
/// [..] body bytes (remainder)
/// ```
///
/// Switch to bincode/postcard if this grows another field — the manual
/// layout is fine for three.
fn join_payload(title: &str, body: &str, tags: &[String]) -> Vec<u8> {
    let tb = title.as_bytes();
    let bb = body.as_bytes();
    let mut out = Vec::with_capacity(1 + 4 + tb.len() + 2 + bb.len());
    out.push(PAYLOAD_V1);
    let title_len = u32::try_from(tb.len()).expect("title under 4GB");
    out.extend_from_slice(&title_len.to_be_bytes());
    out.extend_from_slice(tb);
    let tag_count = u16::try_from(tags.len()).expect("under 65536 tags");
    out.extend_from_slice(&tag_count.to_be_bytes());
    for tag in tags {
        let gb = tag.as_bytes();
        let tag_len = u16::try_from(gb.len()).expect("tag under 64KB");
        out.extend_from_slice(&tag_len.to_be_bytes());
        out.extend_from_slice(gb);
    }
    out.extend_from_slice(bb);
    out
}

fn split_payload(bytes: &[u8]) -> Option<(String, String, Vec<String>)> {
    match bytes.first() {
        Some(&PAYLOAD_V1) => split_payload_v1(&bytes[1..]),
        // Legacy: no version byte, no tags. (A pre-tags vault.)
        _ => split_payload_legacy(bytes).map(|(t, b)| (t, b, Vec::new())),
    }
}

/// Parse the v1 body (the slice *after* the version byte).
fn split_payload_v1(bytes: &[u8]) -> Option<(String, String, Vec<String>)> {
    let mut pos = 0usize;
    let title_len = read_u32(bytes, &mut pos)? as usize;
    let title = read_str(bytes, &mut pos, title_len)?;

    let tag_count = read_u16(bytes, &mut pos)? as usize;
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        let tag_len = read_u16(bytes, &mut pos)? as usize;
        tags.push(read_str(bytes, &mut pos, tag_len)?);
    }

    // Whatever remains is the body.
    let body = String::from_utf8(bytes[pos..].to_vec()).ok()?;
    Some((title, body, tags))
}

/// Parse the pre-tags layout: 4-byte BE title length, title, then body.
fn split_payload_legacy(bytes: &[u8]) -> Option<(String, String)> {
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

fn read_u16(bytes: &[u8], pos: &mut usize) -> Option<u16> {
    let end = pos.checked_add(2)?;
    let v = u16::from_be_bytes(bytes.get(*pos..end)?.try_into().ok()?);
    *pos = end;
    Some(v)
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let end = pos.checked_add(4)?;
    let v = u32::from_be_bytes(bytes.get(*pos..end)?.try_into().ok()?);
    *pos = end;
    Some(v)
}

fn read_str(bytes: &[u8], pos: &mut usize, len: usize) -> Option<String> {
    let end = pos.checked_add(len)?;
    let s = String::from_utf8(bytes.get(*pos..end)?.to_vec()).ok()?;
    *pos = end;
    Some(s)
}

const _ASSERT_KEY_SIZE: usize = KEY_SIZE;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{derive_key, random_salt};
    use crate::storage::VaultStorage;
    use std::path::PathBuf;

    fn tmp_db_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "knot-state-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn new_setup_starts_in_setup_phase() {
        let s = AppState::new_setup();
        assert!(matches!(s.phase, Phase::Setup { error: None }));
        assert_eq!(s.next_id, 1);
        // Salt is a placeholder until complete_setup overwrites it.
        assert_eq!(s.salt, [0u8; SALT_SIZE]);
    }

    #[test]
    fn complete_setup_installs_salt_and_unlocks() {
        let path = tmp_db_path();
        let salt = random_salt();
        let key = derive_key(b"setup-test-pw", &salt);
        let storage = VaultStorage::open(&path, &key).expect("open fresh vault");

        let mut state = AppState::new_setup();
        state.complete_setup(salt, key, Vec::new(), storage);

        assert_eq!(state.salt, salt, "the generated salt must be installed");
        assert!(
            matches!(state.phase, Phase::Unlocked { .. }),
            "setup completion must land in Unlocked"
        );
        // Empty vault → first note id allocation starts at 1.
        assert_eq!(state.next_id, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn payload_round_trips_title_body_and_tags() {
        let tags = vec!["work".to_string(), "日本語".to_string()];
        let bytes = join_payload("My Title", "Body\nwith newline", &tags);
        // v1 always carries the version marker so legacy parsing is skipped.
        assert_eq!(bytes[0], PAYLOAD_V1);
        let (title, body, got_tags) = split_payload(&bytes).expect("v1 payload parses");
        assert_eq!(title, "My Title");
        assert_eq!(body, "Body\nwith newline");
        assert_eq!(got_tags, tags);
    }

    #[test]
    fn payload_round_trips_with_no_tags() {
        let bytes = join_payload("t", "b", &[]);
        let (title, body, tags) = split_payload(&bytes).expect("parses");
        assert_eq!((title.as_str(), body.as_str()), ("t", "b"));
        assert!(tags.is_empty());
    }

    #[test]
    fn legacy_payload_without_version_byte_still_reads() {
        // Reproduce the pre-tags on-disk format by hand: 4-byte BE title
        // length, title, body — no leading version byte.
        let title = "Old Note";
        let body = "decrypted from a pre-tags vault";
        let mut legacy = Vec::new();
        legacy.extend_from_slice(&(title.len() as u32).to_be_bytes());
        legacy.extend_from_slice(title.as_bytes());
        legacy.extend_from_slice(body.as_bytes());
        // High byte of a short title length is 0, never PAYLOAD_V1.
        assert_eq!(legacy[0], 0);

        let (got_title, got_body, tags) = split_payload(&legacy).expect("legacy parses");
        assert_eq!(got_title, title);
        assert_eq!(got_body, body);
        assert!(tags.is_empty(), "legacy notes have no tags");
    }

    #[test]
    fn normalize_tag_lowercases_and_trims() {
        assert_eq!(normalize_tag("  Work "), "work");
        assert_eq!(normalize_tag("RUST"), "rust");
        assert_eq!(normalize_tag("   "), "");
    }

    fn unlocked_state_with_one_note() -> AppState {
        let notes = vec![Note {
            id: 1,
            title: "n".into(),
            body: "b".into(),
            tags: Vec::new(),
        }];
        AppState {
            salt: [0u8; SALT_SIZE],
            next_id: 2,
            phase: Phase::Unlocked {
                dek: derive_key(b"x", &[0u8; SALT_SIZE]),
                notes,
                selected: Some(1),
                storage: open_tmp_storage(),
                dirty: HashSet::new(),
                filter_tags: Vec::new(),
            },
        }
    }

    fn unlocked_state_with_two_tagged_notes() -> AppState {
        let notes = vec![
            Note {
                id: 1,
                title: "a".into(),
                body: String::new(),
                tags: vec!["work".into()],
            },
            Note {
                id: 2,
                title: "b".into(),
                body: String::new(),
                tags: vec!["personal".into()],
            },
        ];
        AppState {
            salt: [0u8; SALT_SIZE],
            next_id: 3,
            phase: Phase::Unlocked {
                dek: derive_key(b"x", &[0u8; SALT_SIZE]),
                notes,
                selected: Some(1),
                storage: open_tmp_storage(),
                dirty: HashSet::new(),
                filter_tags: Vec::new(),
            },
        }
    }

    fn open_tmp_storage() -> VaultStorage {
        let path = tmp_db_path();
        let key = derive_key(b"x", &[0u8; SALT_SIZE]);
        VaultStorage::open(&path, &key).expect("open tmp vault")
    }

    #[test]
    fn add_tag_normalizes_dedupes_and_marks_dirty() {
        let mut s = unlocked_state_with_one_note();
        assert!(s.add_tag_to_selected("  Work "));
        // Duplicate after normalization is rejected.
        assert!(!s.add_tag_to_selected("WORK"));
        // Empty input is rejected.
        assert!(!s.add_tag_to_selected("   "));
        assert!(s.add_tag_to_selected("rust"));

        assert_eq!(
            s.selected_tags(),
            vec!["work".to_string(), "rust".to_string()]
        );
        if let Phase::Unlocked { dirty, .. } = &s.phase {
            assert!(dirty.contains(&1), "an inserted tag marks the note dirty");
        } else {
            panic!("expected Unlocked");
        }
    }

    #[test]
    fn remove_tag_and_all_tags_union() {
        let mut s = unlocked_state_with_one_note();
        s.add_tag_to_selected("work");
        s.add_tag_to_selected("rust");
        assert!(s.remove_tag_from_selected("work"));
        assert!(
            !s.remove_tag_from_selected("work"),
            "second remove is a no-op"
        );
        assert_eq!(s.selected_tags(), vec!["rust".to_string()]);
        // all_tags is the sorted union across notes (one note here).
        assert_eq!(s.all_tags(), vec!["rust".to_string()]);
    }

    #[test]
    fn note_matches_filter_is_intersection() {
        let tags = vec!["work".to_string(), "rust".to_string()];
        // Empty filter matches anything.
        assert!(note_matches_filter(&tags, &[]));
        // A single present tag matches.
        assert!(note_matches_filter(&tags, &["work".to_string()]));
        // Every filter tag must be present (AND).
        assert!(note_matches_filter(
            &tags,
            &["work".to_string(), "rust".to_string()]
        ));
        // A missing tag fails the match even when others are present.
        assert!(!note_matches_filter(
            &tags,
            &["work".to_string(), "play".to_string()]
        ));
        assert!(!note_matches_filter(&tags, &["play".to_string()]));
    }

    #[test]
    fn toggle_filter_normalizes_and_round_trips() {
        let mut s = unlocked_state_with_one_note();
        assert!(!s.is_filtering());
        // Adds (normalized), and reports active.
        assert!(s.toggle_filter_tag("  Work "));
        assert!(s.is_filtering());
        assert!(s.is_filter_active("work"));
        // Toggling the same tag (any case) removes it.
        assert!(!s.toggle_filter_tag("WORK"));
        assert!(!s.is_filtering());
        // Empty input is a no-op.
        assert!(!s.toggle_filter_tag("   "));
    }

    #[test]
    fn filtered_ids_narrow_to_matching_notes() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // No filter → both notes, in stored order.
        assert_eq!(s.filtered_note_ids(), vec![1, 2]);
        assert!(s.has_any_tags());
        // Filter on "work" → only note 1.
        s.toggle_filter_tag("work");
        assert_eq!(s.filtered_note_ids(), vec![1]);
        // Add "personal" (note 1 lacks it) → the intersection empties.
        s.toggle_filter_tag("personal");
        assert!(s.filtered_note_ids().is_empty());
        // note_count ignores the filter.
        assert_eq!(s.note_count(), 2);
    }

    #[test]
    fn clear_and_prune_filter() {
        let mut s = unlocked_state_with_two_tagged_notes();
        s.toggle_filter_tag("work");
        s.toggle_filter_tag("personal");
        s.clear_filter();
        assert!(!s.is_filtering());

        // Filter on a tag, then remove it from the only note carrying it:
        // prune drops the now-orphaned filter tag so the list isn't stuck
        // filtering on a tag with no chip left to toggle it off. (Note 1 is
        // selected in the helper and holds "work".)
        s.toggle_filter_tag("work");
        assert!(s.is_filter_active("work"));
        assert!(s.remove_tag_from_selected("work"));
        s.prune_filter();
        assert!(!s.is_filter_active("work"));
        assert!(!s.is_filtering());
    }
}
