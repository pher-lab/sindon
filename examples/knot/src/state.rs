//! Knot state.
//!
//! Holds a `Phase::Unlocked` carrying the live `VaultStorage` connection
//! alongside the resident plaintext notes, the DEK (data-encryption key
//! that SQLCipher and the per-note `seal` use), and a `dirty` set of note
//! ids that the auto-save tick will flush back to disk. Locking drops the
//! entire `Unlocked` variant — connection, DEK, notes, and dirty set all
//! go in one move, which is what makes "lock" actually release plaintext /
//! re-seal the on-disk DB.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use shroud::render::DecodedImage;
use zeroize::Zeroizing;

use crate::crypto::{KEY_SIZE, MasterKey, NONCE_SIZE, SALT_SIZE, open, seal};
use crate::settings::SortMode;
use crate::storage::{StorageError, VaultStorage};

pub type NoteId = u32;

/// Id of an embedded preview-image attachment. Allocated by the storage
/// layer (highest existing + 1) and embedded in a note body as
/// `![alt](knot-img:<id>)`. Distinct id space from [`NoteId`].
pub type AttachmentId = u32;

/// On-disk shape of a single note row. The ciphertext already contains
/// the XChaCha20-Poly1305 auth tag; the SQLCipher layer adds page-level
/// encryption *on top* of this when these bytes are stored.
pub struct EncryptedNote {
    pub id: NoteId,
    pub nonce: [u8; NONCE_SIZE],
    pub ciphertext: Vec<u8>,
}

/// On-disk shape of one attachment blob (`nonce || ciphertext`). The id is
/// carried by the caller, not the struct, since the only reader
/// ([`AppState::resolve_attachment`]) already knows which id it asked for.
pub struct EncryptedAttachment {
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
    /// Whether the user pinned this note to the top of the sidebar list.
    /// Pinned notes float above the rest regardless of the active
    /// [`SortMode`] (see [`compare_notes`]). Persisted inside the encrypted
    /// payload (a flags byte in the v2 layout) so the pin state, like tags,
    /// never leaks as plaintext metadata.
    pub pinned: bool,
    /// When `Some(unix_secs)`, the note is in the trash (soft-deleted at that
    /// time); `None` means a live note. Trashed notes are hidden from the
    /// normal sidebar list, from tag filters/autocomplete, and from wikilink /
    /// backlink resolution — they only appear in the trash view, from which
    /// they can be restored or permanently deleted. The timestamp also drives
    /// the 30-day auto-purge on unlock (see [`AppState::become_unlocked`]).
    /// Persisted inside the encrypted payload (v3 layout) so deletion state,
    /// like tags and pin, never leaks as plaintext metadata.
    pub deleted_at: Option<u64>,
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
        /// Free-text query the sidebar is narrowing the note list by
        /// (case-insensitive substring over title + body). Combines with
        /// `filter_tags` by intersection — a note shows only if it matches
        /// *both*. Same session-only lifetime as `filter_tags`; never persisted.
        search_query: String,
        /// Whether the sidebar is showing the trash (soft-deleted notes) rather
        /// than the live note list. Pure session UI state, like `filter_tags` /
        /// `search_query`: it resets to `false` on every unlock and is never
        /// persisted. The tag filter and search apply only in the live view.
        trash_view: bool,
        /// Decoded preview images, keyed by attachment id. Populated lazily by
        /// [`AppState::resolve_attachment`] the first time a `knot-img:<id>`
        /// reference is rendered, so only images actually viewed this session
        /// get decrypted into memory. Dropped wholesale on lock with the rest
        /// of `Unlocked` — the decoded pixels (which the framework cannot yet
        /// zeroize) don't outlive the vault session.
        image_cache: HashMap<AttachmentId, Arc<DecodedImage>>,
    },
}

pub struct AppState {
    pub salt: [u8; SALT_SIZE],
    pub next_id: NoteId,
    pub phase: Phase,
    /// Tree index of the sidebar's search `Input`, recorded by `sidebar::build`
    /// so the global Ctrl+F shortcut — registered in `main` before any tree
    /// exists — can focus it. `None` until the vault screen is built. A stale
    /// index after a screen rebuild is harmless: `EventContext::focus` drops
    /// silently if the target is gone, and the handler only acts while unlocked.
    pub search_input_idx: Option<usize>,
    /// Tree index of the editor's find-replace *Find* `Input`, recorded by
    /// `find_replace::build`, so the global Ctrl+H shortcut can focus it when it
    /// opens the bar. Same lifetime / staleness story as
    /// [`Self::search_input_idx`]: `None` until the vault screen builds the
    /// editor, and a stale index after a screen rebuild is harmless
    /// (`EventContext::focus` drops silently).
    pub find_input_idx: Option<usize>,
    /// Tree index of the editor's body `Input`, recorded by `editor::build`, so
    /// the Ctrl+H shortcut can return focus to the body when it *closes* the
    /// find-replace bar. Same lifetime / staleness story as
    /// [`Self::find_input_idx`].
    pub body_input_idx: Option<usize>,
    /// Consecutive failed unlock attempts since the last success. Drives the
    /// escalating lockout (see [`lockout_for`]); reset to 0 on a successful
    /// unlock. In-memory only — a process restart clears it, which matches the
    /// upstream Tauri app. The real cost of a brute-force attempt is Argon2id,
    /// not this counter; the lockout just blunts rapid online guessing within a
    /// session.
    pub failed_attempts: u32,
    /// When set and in the future, the lock screen refuses unlock attempts and
    /// shows a countdown until this instant. Set by [`note_failed_unlock`] once
    /// the attempt count crosses the free-attempt threshold.
    ///
    /// [`note_failed_unlock`]: Self::note_failed_unlock
    pub locked_until: Option<Instant>,
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
            search_input_idx: None,
            find_input_idx: None,
            body_input_idx: None,
            failed_attempts: 0,
            locked_until: None,
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
            search_input_idx: None,
            find_input_idx: None,
            body_input_idx: None,
            failed_attempts: 0,
            locked_until: None,
        }
    }

    /// Transition into `Unlocked` with the given decrypted state. `dek` is
    /// the data-encryption key (unwrapped from `dek.enc` / `recovery.enc`),
    /// not the password. Called by the lock screen, the recovery screen,
    /// and the first-launch setup path.
    pub fn become_unlocked(
        &mut self,
        dek: MasterKey,
        mut notes: Vec<Note>,
        mut storage: VaultStorage,
    ) {
        // Pick the highest existing id + 1 as the next allocation point
        // so a relaunch doesn't recycle ids that crashed before saving.
        // Computed over *all* loaded notes (before the purge below) so a
        // freshly purged id can't be recycled within this session either.
        let max_id = notes.iter().map(|n| n.id).max().unwrap_or(0);
        self.next_id = max_id.saturating_add(1);

        // Auto-purge trash older than the retention window. Best-effort: a
        // failed row delete just leaves the (still-encrypted) note to be swept
        // on a later unlock, so it must not block opening the vault. Done here,
        // the single chokepoint for unlock / recovery / setup, so every entry
        // path purges exactly once.
        let now = unix_now();
        notes.retain(|n| match n.deleted_at {
            Some(t) if now.saturating_sub(t) >= TRASH_RETENTION_SECS => {
                let _ = storage.delete_note(n.id);
                false
            }
            _ => true,
        });

        // Select the first *live* note — a trashed note is never the landing
        // selection (the editor opens on real content, not something in the bin).
        let selected = notes.iter().find(|n| n.deleted_at.is_none()).map(|n| n.id);
        self.phase = Phase::Unlocked {
            dek,
            notes,
            selected,
            storage,
            dirty: HashSet::new(),
            filter_tags: Vec::new(),
            search_query: String::new(),
            trash_view: false,
            image_cache: HashMap::new(),
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

    // ── Unlock lockout ───────────────────────────────────────────────────
    //
    // Brute-force blunting for the lock screen. A run of wrong passwords past
    // a free-attempt threshold starts an escalating cooldown during which the
    // lock screen refuses attempts and counts down. State is in-memory (a
    // restart clears it); see the field docs for why that's acceptable.

    /// Record a failed unlock attempt and, once past the free-attempt
    /// threshold, start (or extend) the lockout. Returns the remaining lockout
    /// duration if one is now in effect, else `None`.
    pub fn note_failed_unlock(&mut self) -> Option<Duration> {
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        match lockout_for(self.failed_attempts) {
            Some(d) => {
                self.locked_until = Some(Instant::now() + d);
                Some(d)
            }
            None => None,
        }
    }

    /// Clear the failed-attempt counter and any active lockout. Called on a
    /// successful unlock so a later wrong password starts counting from zero.
    pub fn reset_unlock_attempts(&mut self) {
        self.failed_attempts = 0;
        self.locked_until = None;
    }

    /// Time left on the current lockout, or `None` if not locked out (never
    /// triggered, or the cooldown has elapsed). Recomputed against "now" on
    /// each call, so the lock screen's reactive status counts it down as the
    /// per-frame tick repaints.
    pub fn lockout_remaining(&self) -> Option<Duration> {
        self.locked_until
            .and_then(|until| until.checked_duration_since(Instant::now()))
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

    /// Append a new note with the given content, select it, and mark it dirty
    /// so the next auto-save tick persists it. Allocates the next id from
    /// `next_id`. Returns the new id, or `None` when locked. Shared by `+ New`
    /// (empty content) and Import (a note read from a `.md` file). The new note
    /// carries no tags — imports intentionally don't round-trip tags, which are
    /// encrypted metadata that must not leak into a plaintext export.
    pub fn add_note(&mut self, title: String, body: String) -> Option<NoteId> {
        // Read the id before borrowing `self.phase`; the counter is bumped
        // (and the note marked dirty) after the borrow ends — NLL releases it
        // past the last field use.
        let id = self.next_id;
        let Phase::Unlocked {
            notes, selected, ..
        } = &mut self.phase
        else {
            return None;
        };
        notes.push(Note {
            id,
            title,
            body,
            tags: Vec::new(),
            pinned: false,
            deleted_at: None,
        });
        *selected = Some(id);
        self.next_id = self.next_id.saturating_add(1);
        // The note now exists, so `mark_dirty` records it for the next
        // auto-save flush (reusing its in-vec membership guard).
        self.mark_dirty(id);
        Some(id)
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
        for note in notes.iter().filter(|n| n.deleted_at.is_none()) {
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

    // ── Pinning ─────────────────────────────────────────────────────────

    /// Whether note `id` is pinned. Drives the pin toggle's filled/outline
    /// glyph in the sidebar. False when locked or the id is unknown.
    pub fn is_pinned(&self, id: NoteId) -> bool {
        matches!(&self.phase, Phase::Unlocked { notes, .. }
            if notes.iter().any(|n| n.id == id && n.pinned))
    }

    /// Flip note `id`'s pinned flag and mark it dirty so the auto-save tick
    /// persists the change (pinned lives in the encrypted payload). Returns
    /// the new state, or `false` when locked / the id is unknown. The caller
    /// rebuilds the list afterwards so the row re-sorts to/from the top.
    pub fn toggle_pin(&mut self, id: NoteId) -> bool {
        let now = if let Phase::Unlocked { notes, .. } = &mut self.phase {
            if let Some(note) = notes.iter_mut().find(|n| n.id == id) {
                note.pinned = !note.pinned;
                note.pinned
            } else {
                return false;
            }
        } else {
            return false;
        };
        self.mark_dirty(id);
        now
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
        matches!(&self.phase, Phase::Unlocked { notes, .. }
            if notes.iter().any(|n| n.deleted_at.is_none() && !n.tags.is_empty()))
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

    /// Note ids to show in the sidebar given the active tag filter *and*
    /// search query, ordered by `sort` with pinned notes floated to the top.
    /// A note shows only if it carries all the active filter tags (see
    /// `note_matches_filter`) *and* its title or body contains the search
    /// query (see `note_matches_search`). With neither active this is every
    /// note. Empty when locked. Ordering is by [`compare_notes`]: pinned
    /// first, then `sort`, with note id as a stable tiebreak.
    pub fn filtered_note_ids(&self, sort: SortMode) -> Vec<NoteId> {
        let Phase::Unlocked {
            notes,
            filter_tags,
            search_query,
            trash_view,
            ..
        } = &self.phase
        else {
            return Vec::new();
        };
        // Trash view: just the soft-deleted notes, most-recently-trashed first.
        // The tag filter and search are live-view concerns and don't apply here.
        if *trash_view {
            let mut matched: Vec<&Note> = notes.iter().filter(|n| n.deleted_at.is_some()).collect();
            matched.sort_by(|a, b| compare_trash(a, b));
            return matched.iter().map(|n| n.id).collect();
        }
        // Live view: skip trashed notes, then narrow by tag filter + search.
        let query = search_query.trim().to_lowercase();
        let mut matched: Vec<&Note> = notes
            .iter()
            .filter(|n| n.deleted_at.is_none())
            .filter(|n| note_matches_filter(&n.tags, filter_tags))
            .filter(|n| note_matches_search(&n.title, &n.body, &query))
            .collect();
        matched.sort_by(|a, b| compare_notes(a, b, sort));
        matched.iter().map(|n| n.id).collect()
    }

    /// Total resident note count including trashed rows (the underlying `notes`
    /// vec length), ignoring the filter. Test-only: lets the trash tests tell
    /// "row removed from the vec" apart from "row merely trashed", which
    /// `live_note_count` can't distinguish. Zero when locked.
    ///
    /// Gated `#[cfg(test)]` — no non-test caller, so it would otherwise trip
    /// `dead_code` in the binary target (and the knot CI clippy gate).
    #[cfg(test)]
    pub fn note_count(&self) -> usize {
        match &self.phase {
            Phase::Unlocked { notes, .. } => notes.len(),
            _ => 0,
        }
    }

    // ── Trash / soft-delete ─────────────────────────────────────────────
    //
    // A delete moves a note to the trash (sets `deleted_at`) rather than
    // dropping its row, so it can be restored. The trash view lists these,
    // offering restore + permanent delete; an unlock auto-purges anything past
    // the retention window. Trashed notes are excluded from the live list,
    // tag filter/autocomplete, search, and wikilink/backlink resolution.

    /// Number of live (non-trashed) notes. Drives the sidebar's "no notes yet"
    /// empty state and the sort-row visibility — both of which are about the
    /// live list, so a vault holding only trashed notes still reads as empty.
    pub fn live_note_count(&self) -> usize {
        match &self.phase {
            Phase::Unlocked { notes, .. } => {
                notes.iter().filter(|n| n.deleted_at.is_none()).count()
            }
            _ => 0,
        }
    }

    /// Number of notes currently in the trash. Drives the "Trash (n)" toggle
    /// label and whether the "Empty trash" action is offered.
    pub fn trash_count(&self) -> usize {
        match &self.phase {
            Phase::Unlocked { notes, .. } => {
                notes.iter().filter(|n| n.deleted_at.is_some()).count()
            }
            _ => 0,
        }
    }

    /// Whether note `id` is in the trash. False when locked or the id is gone.
    pub fn is_trashed(&self, id: NoteId) -> bool {
        matches!(&self.phase, Phase::Unlocked { notes, .. }
            if notes.iter().any(|n| n.id == id && n.deleted_at.is_some()))
    }

    /// Whether the sidebar is currently showing the trash rather than the live
    /// note list. False when locked.
    pub fn is_trash_view(&self) -> bool {
        matches!(&self.phase, Phase::Unlocked { trash_view, .. } if *trash_view)
    }

    /// Switch the sidebar between the live list and the trash. No-op when
    /// locked. The caller rebuilds the sidebar afterwards.
    pub fn set_trash_view(&mut self, on: bool) {
        if let Phase::Unlocked { trash_view, .. } = &mut self.phase {
            *trash_view = on;
        }
    }

    /// Move note `id` to the trash: stamp `deleted_at` with the current time
    /// and mark it dirty so the auto-save tick persists the new state (the flag
    /// rides in the encrypted payload, like pin/tags). If the trashed note was
    /// selected, selection moves to the first remaining live note (or `None`).
    /// Returns whether a note was actually trashed (false when locked, the id
    /// is unknown, or it was already trashed).
    pub fn trash_note(&mut self, id: NoteId) -> bool {
        let now = unix_now();
        let trashed = if let Phase::Unlocked {
            notes, selected, ..
        } = &mut self.phase
        {
            if let Some(note) = notes
                .iter_mut()
                .find(|n| n.id == id && n.deleted_at.is_none())
            {
                note.deleted_at = Some(now);
                if *selected == Some(id) {
                    *selected = notes.iter().find(|n| n.deleted_at.is_none()).map(|n| n.id);
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if trashed {
            self.mark_dirty(id);
        }
        trashed
    }

    /// Restore note `id` from the trash (clear `deleted_at`) and mark it dirty.
    /// Returns whether a note was actually restored (false when locked, the id
    /// is unknown, or it wasn't trashed).
    pub fn restore_note(&mut self, id: NoteId) -> bool {
        let restored = if let Phase::Unlocked { notes, .. } = &mut self.phase {
            if let Some(note) = notes
                .iter_mut()
                .find(|n| n.id == id && n.deleted_at.is_some())
            {
                note.deleted_at = None;
                true
            } else {
                false
            }
        } else {
            false
        };
        if restored {
            self.mark_dirty(id);
        }
        restored
    }

    /// Permanently delete a *trashed* note — removing it from the resident vec
    /// and the on-disk store. Guarded to trashed notes so a stray call can't
    /// hard-delete a live note (the live ✕ path goes through [`Self::trash_note`]).
    /// Errors propagate; the in-memory removal still happens on storage failure
    /// (same defensive stance as the bulk paths). Returns whether a row was
    /// removed.
    pub fn permanent_delete_note(&mut self, id: NoteId) -> Result<bool, StorageError> {
        if !self.is_trashed(id) {
            return Ok(false);
        }
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
            *selected = notes.iter().find(|n| n.deleted_at.is_none()).map(|n| n.id);
        }
        if removed {
            storage.delete_note(id)?;
        }
        Ok(removed)
    }

    /// Permanently delete every trashed note in one sweep. Removes each from the
    /// resident vec and the store; orphaned attachments are reclaimed by the
    /// usual lock-time sweep. Returns the number of notes removed. Storage
    /// errors propagate (the in-memory removal still happens).
    pub fn empty_trash(&mut self) -> Result<usize, StorageError> {
        let Phase::Unlocked {
            notes,
            selected,
            storage,
            dirty,
            ..
        } = &mut self.phase
        else {
            return Ok(0);
        };
        let doomed: Vec<NoteId> = notes
            .iter()
            .filter(|n| n.deleted_at.is_some())
            .map(|n| n.id)
            .collect();
        notes.retain(|n| n.deleted_at.is_none());
        if matches!(selected, Some(id) if doomed.contains(id)) {
            *selected = notes.iter().find(|n| n.deleted_at.is_none()).map(|n| n.id);
        }
        let mut first_err: Option<StorageError> = None;
        for id in &doomed {
            dirty.remove(id);
            if let Err(e) = storage.delete_note(*id) {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(doomed.len()),
        }
    }

    /// Duplicate a *live* note: a deep copy with a fresh id, a disambiguated
    /// "{title} ({suffix})" title, copied tags, and deep-copied attachments
    /// (each referenced image is decrypted and re-sealed under a fresh nonce as
    /// a new attachment, and the body's `knot-img:` refs are rewritten to the
    /// new ids). The copy is unpinned and live regardless of the source's pin
    /// state — matching the upstream Tauri app. Selects the new note and marks
    /// it dirty. Returns the new id, or `None` when locked or the source is
    /// missing / trashed (a note in the bin can't be duplicated).
    ///
    /// `copy_suffix` is supplied by the caller so the title respects the user's
    /// locale ("copy" / "コピー"); an empty suffix falls back to `"copy"`.
    pub fn duplicate_note(&mut self, id: NoteId, copy_suffix: &str) -> Option<NoteId> {
        // Read the next id before borrowing `self.phase`; the counter is bumped
        // after the borrow's last field use (NLL), same idiom as `add_note`.
        let new_id = self.next_id;
        let Phase::Unlocked {
            dek,
            notes,
            selected,
            storage,
            dirty,
            ..
        } = &mut self.phase
        else {
            return None;
        };

        // Source must exist and be live — a trashed note isn't duplicated.
        let src = notes
            .iter()
            .find(|n| n.id == id && n.deleted_at.is_none())?;
        let src_title = src.title.clone();
        let src_body = src.body.clone();
        let src_tags = src.tags.clone();

        // Resolve a unique "{title} ({suffix})" against existing live titles
        // (case-insensitive), bumping to "({suffix} 2)", "({suffix} 3)", …
        let suffix = match copy_suffix.trim() {
            "" => "copy",
            s => s,
        };
        let taken: HashSet<String> = notes
            .iter()
            .filter(|n| n.deleted_at.is_none())
            .map(|n| n.title.to_lowercase())
            .collect();
        let new_title = next_available_title(&src_title, suffix, &taken);

        // Deep-copy each referenced attachment (decrypt + re-seal under a fresh
        // nonce), building an old→new id map, then rewrite the body's refs.
        // Attachments that fail to load / decrypt are skipped — their refs in
        // the copy stay pointing at the original id, exactly as upstream does.
        let mut refs: HashSet<AttachmentId> = HashSet::new();
        extract_attachment_refs(&src_body, &mut refs);
        let mut id_map: HashMap<AttachmentId, AttachmentId> = HashMap::new();
        for old in refs {
            let Ok(Some(enc)) = storage.load_attachment(old) else {
                continue;
            };
            let Some(plain) = open(dek, &enc.nonce, &enc.ciphertext) else {
                continue;
            };
            let plain = Zeroizing::new(plain);
            let (nonce, ciphertext) = seal(dek, &plain);
            if let Ok(new_att) = storage.insert_attachment(&nonce, &ciphertext) {
                id_map.insert(old, new_att);
            }
        }
        let new_body = rewrite_attachment_refs(&src_body, &id_map);

        notes.push(Note {
            id: new_id,
            title: new_title,
            body: new_body,
            tags: src_tags,
            pinned: false,
            deleted_at: None,
        });
        *selected = Some(new_id);
        dirty.insert(new_id);

        // Last phase-field use is above — the `&mut self.phase` borrow is
        // released here (NLL), so bumping the id counter is allowed.
        self.next_id = self.next_id.saturating_add(1);
        Some(new_id)
    }

    // ── Sidebar full-text search ────────────────────────────────────────
    //
    // Session-only UI state paralleling the tag filter: a free-text query the
    // note list is narrowed by (case-insensitive substring over title + body).
    // Intersects with the tag filter — a note shows only when it matches both.
    // Lives in `Unlocked`, drops on lock, and is never persisted.

    /// Replace the active search query. Stored verbatim (the match lowercases
    /// at compare time, see `note_matches_search`); an all-whitespace query
    /// reads as "not searching". No-op when locked.
    pub fn set_search_query(&mut self, raw: &str) {
        if let Phase::Unlocked { search_query, .. } = &mut self.phase {
            raw.clone_into(search_query);
        }
    }

    /// Whether a non-empty search query is currently narrowing the note list.
    /// Lets the sidebar phrase its empty-list message ("no notes match your
    /// search") and clears alongside the filter on `+ New`.
    pub fn is_searching(&self) -> bool {
        matches!(&self.phase, Phase::Unlocked { search_query, .. } if !search_query.trim().is_empty())
    }

    /// Clear the search query so every note shows again (subject to any tag
    /// filter still active).
    pub fn clear_search(&mut self) {
        if let Phase::Unlocked { search_query, .. } = &mut self.phase {
            search_query.clear();
        }
    }

    // ── Attachments (embedded preview images) ───────────────────────────

    /// Seal `bytes` under the DEK and store them as a new attachment,
    /// returning the allocated id to embed in a note body as
    /// `![alt](knot-img:<id>)`. Returns `None` when locked, when the input
    /// is empty, when it exceeds [`MAX_ATTACHMENT_BYTES`], or when the write
    /// fails — the caller (the editor's image button) simply doesn't insert
    /// a reference in any of those cases. Caller is expected to have already
    /// validated the bytes decode as an image.
    pub fn add_attachment(&mut self, bytes: &[u8]) -> Option<AttachmentId> {
        if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
            return None;
        }
        let Phase::Unlocked { dek, storage, .. } = &mut self.phase else {
            return None;
        };
        let (nonce, ciphertext) = seal(dek, bytes);
        storage.insert_attachment(&nonce, &ciphertext).ok()
    }

    /// Resolve an attachment id to its decoded pixels, decrypting on first
    /// use and caching the result for the rest of the session. Returns
    /// `None` when locked, when no such attachment exists (a dangling
    /// reference), when decryption fails (tampered DB), or when the bytes
    /// don't decode as a supported image. The intermediate plaintext is
    /// zeroized after decoding — only the `DecodedImage`'s own pixel copy
    /// remains resident.
    pub fn resolve_attachment(&mut self, id: AttachmentId) -> Option<Arc<DecodedImage>> {
        let Phase::Unlocked {
            dek,
            storage,
            image_cache,
            ..
        } = &mut self.phase
        else {
            return None;
        };
        if let Some(img) = image_cache.get(&id) {
            return Some(Arc::clone(img));
        }
        let enc = storage.load_attachment(id).ok()??;
        let plaintext = Zeroizing::new(open(dek, &enc.nonce, &enc.ciphertext)?);
        let decoded = DecodedImage::from_bytes(&plaintext).ok()?;
        image_cache.insert(id, Arc::clone(&decoded));
        Some(decoded)
    }

    /// Delete every stored attachment no live note body still references.
    /// Run at lock time (after the final note rewrite) so deleting a note or
    /// editing out an `![](knot-img:<id>)` reference eventually reclaims the
    /// orphaned ciphertext rather than leaving it as dead weight in the DB.
    /// No-op when locked.
    pub fn prune_orphan_attachments(&mut self) -> Result<(), StorageError> {
        let Phase::Unlocked {
            notes,
            storage,
            image_cache,
            ..
        } = &mut self.phase
        else {
            return Ok(());
        };
        let mut referenced: HashSet<AttachmentId> = HashSet::new();
        for note in notes.iter() {
            extract_attachment_refs(&note.body, &mut referenced);
        }
        for id in storage.all_attachment_ids()? {
            if !referenced.contains(&id) {
                storage.delete_attachment(id)?;
                image_cache.remove(&id);
            }
        }
        Ok(())
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
            let payload = join_payload(
                &note.title,
                &note.body,
                &note.tags,
                note.pinned,
                note.deleted_at,
            );
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
                let payload = join_payload(
                    &note.title,
                    &note.body,
                    &note.tags,
                    note.pinned,
                    note.deleted_at,
                );
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

    /// Drop the unlocked session **without flushing to storage**, closing the
    /// DB connection. Used before a backup restore overwrites the vault files:
    ///
    /// * Flushing here (as [`lock_and_seal`](Self::lock_and_seal) does) would
    ///   re-encrypt the *old* in-memory notes into the DB and clobber the
    ///   just-restored file.
    /// * The open `VaultStorage` connection holds `vault.db` open, which on
    ///   Windows blocks overwriting it — so the connection must close first.
    ///
    /// Replacing the `Unlocked` variant drops `storage` (closing the conn),
    /// the DEK, the notes, and the image cache, all without touching disk.
    pub fn discard_and_lock(&mut self) {
        self.phase = Phase::Locked { error: None };
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
        // Reclaim any attachment no note still references. Best-effort: a
        // failure just leaves (still-encrypted) dead ciphertext to be swept
        // next lock, so it must not block the lock either.
        if let Err(e) = self.prune_orphan_attachments() {
            eprintln!("knot: failed to prune orphan attachments on lock: {}", e);
        }
        // Replace the variant; key, storage (closing conn), notes,
        // dirty, image cache all drop here.
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
        let (title, body, tags, pinned, deleted_at) = split_payload(&pt)?;
        out.push(Note {
            id: enc.id,
            title,
            body,
            tags,
            pinned,
            deleted_at,
        });
    }
    Some(out)
}

/// Ordering for the sidebar note list: pinned notes always sort before
/// unpinned ones, then ties break by the active [`SortMode`], then by note
/// id (stable, ascending) so the order is fully deterministic. Pure — takes
/// no app state — so the sort logic is unit-testable without a vault.
fn compare_notes(a: &Note, b: &Note, sort: SortMode) -> Ordering {
    // `true > false`, so reverse to float pinned (true) to the front.
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| match sort {
            SortMode::Created => Ordering::Equal,
            SortMode::TitleAsc => title_key(&a.title).cmp(&title_key(&b.title)),
            SortMode::TitleDesc => title_key(&b.title).cmp(&title_key(&a.title)),
        })
        .then_with(|| a.id.cmp(&b.id))
}

/// Case-insensitive title sort key. An empty (untitled) note sorts as an
/// empty string — i.e. first under A–Z — which keeps brand-new notes visible
/// at the top of an alphabetical list until they're named.
fn title_key(title: &str) -> String {
    title.to_lowercase()
}

/// Ordering for the trash view: most-recently-trashed first, with note id as a
/// stable tiebreak. Independent of the live list's [`SortMode`] / pin float —
/// in the bin, "what did I just delete?" is the useful order. A `None`
/// `deleted_at` (shouldn't occur here — the caller pre-filters to trashed
/// notes) sorts last.
fn compare_trash(a: &Note, b: &Note) -> Ordering {
    b.deleted_at
        .cmp(&a.deleted_at)
        .then_with(|| a.id.cmp(&b.id))
}

/// Number of wrong passwords allowed before any cooldown kicks in. A typo or
/// two shouldn't lock anyone out, but a sustained run should.
const FREE_UNLOCK_ATTEMPTS: u32 = 5;

/// Lockout duration after `attempts` consecutive failures, or `None` while
/// still within the free allowance. Escalates from 15 s, doubling each
/// further failure, capped at 5 minutes — so a persistent guesser hits an
/// ever-growing wall while a fat-fingered user barely notices. Pure (no
/// clock), so the policy is unit-testable.
fn lockout_for(attempts: u32) -> Option<Duration> {
    if attempts < FREE_UNLOCK_ATTEMPTS {
        return None;
    }
    // 0 at the first over-threshold failure, then 1, 2, … — the doubling power.
    let over = attempts - FREE_UNLOCK_ATTEMPTS;
    // 15 << over = 15 * 2^over; `checked_shl` guards a pathological shift width
    // (≥ 64 → None → saturate to the cap below).
    let secs = 15u64.checked_shl(over).unwrap_or(u64::MAX).min(300);
    Some(Duration::from_secs(secs))
}

/// How long a note lingers in the trash before an unlock auto-purges it.
/// 30 days, matching the upstream Tauri app's `purge_old_trash` cutoff — long
/// enough to undo a mistaken delete, short enough that the bin doesn't grow
/// unbounded.
const TRASH_RETENTION_SECS: u64 = 30 * 24 * 60 * 60;

/// Current Unix time in whole seconds (saturating to 0 before the epoch — a
/// clock set before 1970 just reads as "live" for trash purposes). Used to
/// stamp `deleted_at` and to measure trash age at unlock.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

/// True when `title` or `body` contains `query` (case-insensitive substring).
/// `query` is expected already trimmed + lowercased by the caller
/// ([`AppState::filtered_note_ids`]); an empty query matches everything, so an
/// inactive search never hides a note. Searches plaintext content only — tags
/// are handled by the separate tag filter.
pub fn note_matches_search(title: &str, body: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    title.to_lowercase().contains(query) || body.to_lowercase().contains(query)
}

/// Upper bound on a single stored attachment. Guards the DB and memory
/// against a pathological multi-hundred-MB blob; ordinary screenshots and
/// photos sit comfortably under this.
const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

/// Internal markdown URL scheme marking an embedded attachment reference:
/// `![alt](knot-img:<id>)`. The preview resolves it against the encrypted
/// attachment store; every other src (http/https/file/data/…) is treated as
/// an inert external image and never fetched or read.
pub const IMG_SCHEME: &str = "knot-img:";

/// Parse a `knot-img:<id>` reference into its attachment id. Returns `None`
/// for any other src, which routes external/unknown images to the inert path.
pub fn parse_attachment_ref(dest: &str) -> Option<AttachmentId> {
    dest.strip_prefix(IMG_SCHEME)?.parse::<AttachmentId>().ok()
}

/// Collect every attachment id `body` references into `out`. Used by the
/// orphan sweep to learn which stored attachments are still live. Scans for
/// the `knot-img:` scheme followed by ASCII digits, so it matches the ids
/// inside `![alt](knot-img:<id>)` without a full markdown parse.
pub fn extract_attachment_refs(body: &str, out: &mut HashSet<AttachmentId>) {
    let mut rest = body;
    while let Some(pos) = rest.find(IMG_SCHEME) {
        // Advance past the scheme we just found before scanning digits, so
        // the same occurrence can never be re-matched (no infinite loop).
        let after = &rest[pos + IMG_SCHEME.len()..];
        let digit_len = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if let Ok(id) = after[..digit_len].parse::<AttachmentId>() {
            out.insert(id);
        }
        rest = &after[digit_len..];
    }
}

/// Find the first available "{base} ({suffix})" title not already in `taken`
/// (compared case-insensitively — `taken` holds lowercased titles), falling
/// back to "{base} ({suffix} 2)", "({suffix} 3)", … on collision. Pure, so the
/// disambiguation is unit-testable. Mirrors the upstream Tauri duplicate-title
/// logic.
fn next_available_title(base: &str, suffix: &str, taken: &HashSet<String>) -> String {
    let first = format!("{} ({})", base, suffix);
    if !taken.contains(&first.to_lowercase()) {
        return first;
    }
    for n in 2u32.. {
        let candidate = format!("{} ({} {})", base, suffix, n);
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("the u32 range yields a free title long before exhaustion")
}

/// Rewrite every `knot-img:<id>` reference in `body` whose id appears in
/// `id_map` to the mapped id, leaving unmapped refs (and all other text)
/// untouched. Digit-boundary aware so `knot-img:1` is never matched inside
/// `knot-img:12` (a naive `str::replace` would corrupt it). Returns `body`
/// unchanged when the map is empty (the common no-attachment case).
fn rewrite_attachment_refs(body: &str, id_map: &HashMap<AttachmentId, AttachmentId>) -> String {
    if id_map.is_empty() {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(pos) = rest.find(IMG_SCHEME) {
        // Emit everything up to and including the scheme, then handle the digits.
        let (head, after) = rest.split_at(pos + IMG_SCHEME.len());
        out.push_str(head);
        let digit_len = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        let (digits, tail) = after.split_at(digit_len);
        match digits.parse::<AttachmentId>() {
            Ok(old) if id_map.contains_key(&old) => {
                out.push_str(&id_map[&old].to_string());
            }
            // Unmapped id, or no digits at all — keep the original text verbatim.
            _ => out.push_str(digits),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// First byte of a versioned payload. The legacy (pre-tags) format begins
/// with the big-endian *high* byte of the title length, which is `0` for any
/// realistic title (< 16 MiB), so a leading `1`/`2` unambiguously flags a
/// newer layout.
///
/// Version history:
///
/// - v1 — added tags (a `[2] tag_count` block before the body).
/// - v2 — added a flags byte (bit 0 = pinned) right after the version.
/// - v3 — added an 8-byte `deleted_at` field (0 = live) after the flags, for
///   the trash / soft-delete state.
///
/// Old vaults stay readable (v2 → `deleted_at = None`, v1 → also `pinned =
/// false`, legacy → no tags either); new writes always use v3.
const PAYLOAD_V1: u8 = 1;
const PAYLOAD_V2: u8 = 2;
const PAYLOAD_V3: u8 = 3;

/// Per-note flag bits carried in the v2 flags byte.
const FLAG_PINNED: u8 = 0b0000_0001;

/// Payload encoding (v3):
///
/// ```text
/// [1]  version = PAYLOAD_V3
/// [1]  flags (bit 0 = pinned; other bits reserved, written 0)
/// [8]  deleted_at, big-endian unix seconds (0 = live / not trashed)
/// [4]  title length, big-endian
/// [N]  title bytes
/// [2]  tag count, big-endian
///   repeated: [2] tag length BE, [M] tag bytes
/// [..] body bytes (remainder)
/// ```
///
/// Switch to bincode/postcard if this grows much further — the manual layout
/// is still fine for these few fields plus a flags byte.
fn join_payload(
    title: &str,
    body: &str,
    tags: &[String],
    pinned: bool,
    deleted_at: Option<u64>,
) -> Vec<u8> {
    let tb = title.as_bytes();
    let bb = body.as_bytes();
    let mut out = Vec::with_capacity(1 + 1 + 8 + 4 + tb.len() + 2 + bb.len());
    out.push(PAYLOAD_V3);
    out.push(if pinned { FLAG_PINNED } else { 0 });
    // 0 is the "live" sentinel — no note is ever trashed at the Unix epoch, so
    // it can't collide with a real deletion timestamp.
    out.extend_from_slice(&deleted_at.unwrap_or(0).to_be_bytes());
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

/// Decoded note fields: title, body, tags, pinned, deleted_at. Older on-disk
/// versions fill the newer fields with their defaults (v2 → `deleted_at =
/// None`, v1 → also `pinned = false`, legacy → no tags either).
type DecodedPayload = (String, String, Vec<String>, bool, Option<u64>);

fn split_payload(bytes: &[u8]) -> Option<DecodedPayload> {
    match bytes.first() {
        Some(&PAYLOAD_V3) => split_payload_v3(&bytes[1..]),
        // v2: pinned flag but no deleted_at → never trashed.
        Some(&PAYLOAD_V2) => split_payload_v2(&bytes[1..]).map(|(t, b, g, p)| (t, b, g, p, None)),
        // v1: tags but no flags byte → never pinned, never trashed.
        Some(&PAYLOAD_V1) => split_payload_v1(&bytes[1..]).map(|(t, b, g)| (t, b, g, false, None)),
        // Legacy: no version byte, no tags, never pinned. (A pre-tags vault.)
        _ => split_payload_legacy(bytes).map(|(t, b)| (t, b, Vec::new(), false, None)),
    }
}

/// Parse the v3 body (the slice *after* the version byte): a flags byte, an
/// 8-byte `deleted_at`, then the same title/tags/body layout as v2.
fn split_payload_v3(bytes: &[u8]) -> Option<DecodedPayload> {
    let mut pos = 0usize;
    let flags = *bytes.get(pos)?;
    pos += 1;
    let pinned = flags & FLAG_PINNED != 0;

    let raw_deleted = read_u64(bytes, &mut pos)?;
    let deleted_at = (raw_deleted != 0).then_some(raw_deleted);

    let title_len = read_u32(bytes, &mut pos)? as usize;
    let title = read_str(bytes, &mut pos, title_len)?;

    let tag_count = read_u16(bytes, &mut pos)? as usize;
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        let tag_len = read_u16(bytes, &mut pos)? as usize;
        tags.push(read_str(bytes, &mut pos, tag_len)?);
    }

    let body = String::from_utf8(bytes[pos..].to_vec()).ok()?;
    Some((title, body, tags, pinned, deleted_at))
}

/// Parse the v2 body (the slice *after* the version byte): a flags byte, then
/// the same title/tags/body layout as v1.
fn split_payload_v2(bytes: &[u8]) -> Option<(String, String, Vec<String>, bool)> {
    let mut pos = 0usize;
    let flags = *bytes.get(pos)?;
    pos += 1;
    let pinned = flags & FLAG_PINNED != 0;

    let title_len = read_u32(bytes, &mut pos)? as usize;
    let title = read_str(bytes, &mut pos, title_len)?;

    let tag_count = read_u16(bytes, &mut pos)? as usize;
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        let tag_len = read_u16(bytes, &mut pos)? as usize;
        tags.push(read_str(bytes, &mut pos, tag_len)?);
    }

    let body = String::from_utf8(bytes[pos..].to_vec()).ok()?;
    Some((title, body, tags, pinned))
}

/// Parse the v1 body (the slice *after* the version byte). No flags byte.
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

fn read_u64(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let end = pos.checked_add(8)?;
    let v = u64::from_be_bytes(bytes.get(*pos..end)?.try_into().ok()?);
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
    fn payload_round_trips_title_body_tags_and_pin() {
        let tags = vec!["work".to_string(), "日本語".to_string()];
        let bytes = join_payload(
            "My Title",
            "Body\nwith newline",
            &tags,
            true,
            Some(1_700_000_000),
        );
        // v3 always carries the version marker so legacy parsing is skipped.
        assert_eq!(bytes[0], PAYLOAD_V3);
        let (title, body, got_tags, pinned, deleted_at) =
            split_payload(&bytes).expect("v3 payload parses");
        assert_eq!(title, "My Title");
        assert_eq!(body, "Body\nwith newline");
        assert_eq!(got_tags, tags);
        assert!(pinned, "the pinned flag round-trips");
        assert_eq!(deleted_at, Some(1_700_000_000), "deleted_at round-trips");
    }

    #[test]
    fn payload_round_trips_with_no_tags_unpinned_live() {
        let bytes = join_payload("t", "b", &[], false, None);
        let (title, body, tags, pinned, deleted_at) = split_payload(&bytes).expect("parses");
        assert_eq!((title.as_str(), body.as_str()), ("t", "b"));
        assert!(tags.is_empty());
        assert!(!pinned);
        assert_eq!(deleted_at, None, "a live note has no deleted_at");
    }

    #[test]
    fn legacy_v2_payload_reads_back_live() {
        // A v2 note (flags byte but no deleted_at) must read back as live —
        // the deletion field didn't exist when it was written.
        let mut v2 = vec![PAYLOAD_V2, FLAG_PINNED];
        let title = "V2 Note";
        v2.extend_from_slice(&(title.len() as u32).to_be_bytes());
        v2.extend_from_slice(title.as_bytes());
        v2.extend_from_slice(&0u16.to_be_bytes()); // zero tags
        v2.extend_from_slice(b"body");

        let (got_title, got_body, tags, pinned, deleted_at) =
            split_payload(&v2).expect("v2 parses");
        assert_eq!(got_title, title);
        assert_eq!(got_body, "body");
        assert!(tags.is_empty());
        assert!(pinned, "the v2 pinned flag still round-trips");
        assert_eq!(deleted_at, None, "a v2 note reads back live");
    }

    #[test]
    fn legacy_v1_payload_reads_back_unpinned() {
        // Reproduce the v1 (tags, no flags byte) on-disk format by hand: a
        // PAYLOAD_V1 marker, then 4-byte BE title length, title, tag count, body.
        let title = "V1 Note";
        let body = "from a pre-pin vault";
        let mut v1 = vec![PAYLOAD_V1];
        v1.extend_from_slice(&(title.len() as u32).to_be_bytes());
        v1.extend_from_slice(title.as_bytes());
        v1.extend_from_slice(&0u16.to_be_bytes()); // zero tags
        v1.extend_from_slice(body.as_bytes());

        let (got_title, got_body, tags, pinned, deleted_at) =
            split_payload(&v1).expect("v1 parses");
        assert_eq!(got_title, title);
        assert_eq!(got_body, body);
        assert!(tags.is_empty());
        assert!(!pinned, "a v1 note (no flags byte) reads back unpinned");
        assert_eq!(deleted_at, None, "a v1 note reads back live");
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
        // High byte of a short title length is 0, never a version marker.
        assert_eq!(legacy[0], 0);

        let (got_title, got_body, tags, pinned, deleted_at) =
            split_payload(&legacy).expect("legacy parses");
        assert_eq!(got_title, title);
        assert_eq!(got_body, body);
        assert!(tags.is_empty(), "legacy notes have no tags");
        assert!(!pinned, "legacy notes are never pinned");
        assert_eq!(deleted_at, None, "legacy notes read back live");
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
            pinned: false,
            deleted_at: None,
        }];
        AppState {
            salt: [0u8; SALT_SIZE],
            next_id: 2,
            search_input_idx: None,
            find_input_idx: None,
            body_input_idx: None,
            failed_attempts: 0,
            locked_until: None,
            phase: Phase::Unlocked {
                dek: derive_key(b"x", &[0u8; SALT_SIZE]),
                notes,
                selected: Some(1),
                storage: open_tmp_storage(),
                dirty: HashSet::new(),
                filter_tags: Vec::new(),
                search_query: String::new(),
                trash_view: false,
                image_cache: HashMap::new(),
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
                pinned: false,
                deleted_at: None,
            },
            Note {
                id: 2,
                title: "b".into(),
                body: String::new(),
                tags: vec!["personal".into()],
                pinned: false,
                deleted_at: None,
            },
        ];
        AppState {
            salt: [0u8; SALT_SIZE],
            next_id: 3,
            search_input_idx: None,
            find_input_idx: None,
            body_input_idx: None,
            failed_attempts: 0,
            locked_until: None,
            phase: Phase::Unlocked {
                dek: derive_key(b"x", &[0u8; SALT_SIZE]),
                notes,
                selected: Some(1),
                storage: open_tmp_storage(),
                dirty: HashSet::new(),
                filter_tags: Vec::new(),
                search_query: String::new(),
                trash_view: false,
                image_cache: HashMap::new(),
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
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
        assert!(s.has_any_tags());
        // Filter on "work" → only note 1.
        s.toggle_filter_tag("work");
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1]);
        // Add "personal" (note 1 lacks it) → the intersection empties.
        s.toggle_filter_tag("personal");
        assert!(s.filtered_note_ids(SortMode::Created).is_empty());
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

    // ── Trash / soft-delete ─────────────────────────────────────────────

    #[test]
    fn trash_hides_note_from_live_list_and_moves_selection() {
        let mut s = unlocked_state_with_two_tagged_notes(); // ids 1,2; selected 1
        assert!(s.trash_note(1), "first trash succeeds");
        assert!(
            !s.trash_note(1),
            "re-trashing an already-trashed note is a no-op"
        );

        assert!(s.is_trashed(1));
        assert!(!s.is_trashed(2));
        // The live list now shows only the surviving note.
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![2]);
        assert_eq!(s.live_note_count(), 1);
        assert_eq!(s.trash_count(), 1);
        // Selection moved off the trashed note to the remaining live one.
        if let Phase::Unlocked { selected, .. } = &s.phase {
            assert_eq!(*selected, Some(2));
        } else {
            panic!("expected Unlocked");
        }
        // Trashing marks the note dirty so the new state persists.
        if let Phase::Unlocked { dirty, .. } = &s.phase {
            assert!(dirty.contains(&1));
        }
    }

    #[test]
    fn trashed_note_drops_out_of_tags_and_search() {
        let mut s = unlocked_state_with_two_tagged_notes(); // 1="work", 2="personal"
        // Before: both tags visible.
        assert_eq!(
            s.all_tags(),
            vec!["personal".to_string(), "work".to_string()]
        );
        assert!(s.has_any_tags());

        s.trash_note(1); // removes the only "work" note
        assert_eq!(
            s.all_tags(),
            vec!["personal".to_string()],
            "a trashed note's tags no longer populate the filter set"
        );
        assert!(s.has_any_tags(), "note 2 still carries a tag");
    }

    #[test]
    fn trash_view_lists_only_trashed_notes() {
        let mut s = unlocked_state_with_two_tagged_notes();
        s.trash_note(1);
        // Live view: only note 2.
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![2]);
        // Trash view: only note 1.
        s.set_trash_view(true);
        assert!(s.is_trash_view());
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1]);
    }

    #[test]
    fn restore_brings_a_note_back_to_the_live_list() {
        let mut s = unlocked_state_with_two_tagged_notes();
        s.trash_note(1);
        assert!(s.restore_note(1), "restore succeeds for a trashed note");
        assert!(!s.restore_note(1), "restoring a live note is a no-op");
        assert!(!s.is_trashed(1));
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
        assert_eq!(s.trash_count(), 0);
    }

    #[test]
    fn permanent_delete_is_guarded_to_trashed_notes() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // A live note can't be permanently deleted — the live ✕ trashes instead.
        assert!(
            !s.permanent_delete_note(1).expect("no storage error"),
            "permanent delete refuses a live note"
        );
        assert_eq!(s.note_count(), 2, "the live note is untouched");

        // Once trashed, it can be permanently removed.
        s.trash_note(1);
        assert!(s.permanent_delete_note(1).expect("delete ok"));
        assert_eq!(s.note_count(), 1, "the row is gone from the vec");
        assert!(!s.is_trashed(1));
    }

    #[test]
    fn empty_trash_removes_every_trashed_note() {
        let mut s = unlocked_state_with_two_tagged_notes();
        s.trash_note(1);
        s.trash_note(2);
        assert_eq!(s.live_note_count(), 0);
        let removed = s.empty_trash().expect("empty ok");
        assert_eq!(removed, 2);
        assert_eq!(s.note_count(), 0, "the vault is now empty");
        // Selection fell through to None once the last note was trashed.
        if let Phase::Unlocked { selected, .. } = &s.phase {
            assert_eq!(*selected, None);
        } else {
            panic!("expected Unlocked");
        }
    }

    // ── Duplicate ───────────────────────────────────────────────────────

    /// Title of note `id` in an unlocked state, for the duplicate tests.
    fn title_of(s: &AppState, id: NoteId) -> String {
        match &s.phase {
            Phase::Unlocked { notes, .. } => {
                notes.iter().find(|n| n.id == id).unwrap().title.clone()
            }
            _ => unreachable!("expected Unlocked"),
        }
    }

    #[test]
    fn duplicate_copies_content_tags_unpinned_and_selects() {
        let mut s = unlocked_state_with_two_tagged_notes(); // id1 "a"/work, id2 "b"/personal
        // Give the source a body and pin it — the copy takes the body, not the pin.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].body = "hello world".into();
        }
        s.toggle_pin(1);
        let next_before = s.next_id;

        let new_id = s.duplicate_note(1, "copy").expect("duplicate succeeds");
        assert_eq!(new_id, next_before, "uses the current next_id");
        assert_eq!(s.next_id, next_before + 1, "bumps next_id");

        if let Phase::Unlocked {
            notes,
            selected,
            dirty,
            ..
        } = &s.phase
        {
            let dup = notes.iter().find(|n| n.id == new_id).expect("copy present");
            assert_eq!(dup.title, "a (copy)");
            assert_eq!(dup.body, "hello world");
            assert_eq!(dup.tags, vec!["work".to_string()]);
            assert!(!dup.pinned, "the copy is never pinned");
            assert!(dup.deleted_at.is_none(), "the copy is live");
            assert_eq!(*selected, Some(new_id), "the copy is selected");
            assert!(dirty.contains(&new_id), "the copy is dirty for persistence");
        } else {
            panic!("expected Unlocked");
        }
    }

    #[test]
    fn duplicate_disambiguates_title_on_collision() {
        let mut s = unlocked_state_with_two_tagged_notes();
        let first = s.duplicate_note(1, "copy").unwrap();
        let second = s.duplicate_note(1, "copy").unwrap();
        assert_eq!(title_of(&s, first), "a (copy)");
        assert_eq!(title_of(&s, second), "a (copy 2)");
    }

    #[test]
    fn duplicate_blank_suffix_falls_back_to_copy() {
        let mut s = unlocked_state_with_two_tagged_notes();
        let id = s.duplicate_note(1, "   ").unwrap();
        assert_eq!(title_of(&s, id), "a (copy)");
    }

    #[test]
    fn duplicate_rejects_a_trashed_note() {
        let mut s = unlocked_state_with_two_tagged_notes();
        s.trash_note(1);
        assert_eq!(
            s.duplicate_note(1, "copy"),
            None,
            "a trashed note can't be duplicated"
        );
    }

    #[test]
    fn duplicate_deep_copies_attachments() {
        let mut s = unlocked_state_with_one_note(); // id 1
        let png = tiny_png();
        let att = s.add_attachment(&png).expect("store attachment");
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].body = format!("![pic](knot-img:{att})");
        }

        let dup_id = s.duplicate_note(1, "copy").expect("duplicate succeeds");

        // The copy's body must reference a *different* attachment id.
        let dup_body = match &s.phase {
            Phase::Unlocked { notes, .. } => {
                notes.iter().find(|n| n.id == dup_id).unwrap().body.clone()
            }
            _ => unreachable!(),
        };
        let mut refs = HashSet::new();
        extract_attachment_refs(&dup_body, &mut refs);
        assert_eq!(refs.len(), 1, "the copy references exactly one attachment");
        let new_att = *refs.iter().next().unwrap();
        assert_ne!(new_att, att, "the copy points at a fresh attachment id");

        // Both attachments exist and decode to the same image.
        let orig = s.resolve_attachment(att).expect("original attachment");
        let copy = s.resolve_attachment(new_att).expect("copied attachment");
        assert_eq!(
            (orig.width(), orig.height()),
            (copy.width(), copy.height()),
            "the copied image decodes to the same dimensions"
        );
    }

    // ── Pinning + sort ───────────────────────────────────────────────────

    #[test]
    fn toggle_pin_flips_state_and_marks_dirty() {
        let mut s = unlocked_state_with_one_note();
        assert!(!s.is_pinned(1));
        assert!(s.toggle_pin(1), "first toggle pins");
        assert!(s.is_pinned(1));
        if let Phase::Unlocked { dirty, .. } = &s.phase {
            assert!(
                dirty.contains(&1),
                "pinning marks the note dirty for persistence"
            );
        } else {
            panic!("expected Unlocked");
        }
        assert!(!s.toggle_pin(1), "second toggle unpins");
        assert!(!s.is_pinned(1));
        // An unknown id is a no-op.
        assert!(!s.toggle_pin(999));
    }

    #[test]
    fn pinned_notes_float_to_the_top_regardless_of_sort() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // Give titles so the alphabetical modes have something to order by.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].title = "Banana".into(); // id 1
            notes[1].title = "Apple".into(); // id 2
        }
        // Created (id order): 1, 2.
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
        // A–Z by title: Apple(2) before Banana(1).
        assert_eq!(s.filtered_note_ids(SortMode::TitleAsc), vec![2, 1]);
        // Z–A by title: Banana(1) before Apple(2).
        assert_eq!(s.filtered_note_ids(SortMode::TitleDesc), vec![1, 2]);

        // Pin the note that would otherwise sort last in each mode (Banana/id 1
        // is last under A–Z) — it now leads regardless of sort.
        assert!(s.toggle_pin(1));
        assert_eq!(s.filtered_note_ids(SortMode::TitleAsc), vec![1, 2]);
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
    }

    #[test]
    fn sort_is_stable_by_id_within_equal_keys() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // Equal titles → the Created/title comparators tie, so id breaks it
        // deterministically (ascending), never a random order.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].title = "same".into();
            notes[1].title = "same".into();
        }
        assert_eq!(s.filtered_note_ids(SortMode::TitleAsc), vec![1, 2]);
        assert_eq!(s.filtered_note_ids(SortMode::TitleDesc), vec![1, 2]);
    }

    // ── Unlock lockout ───────────────────────────────────────────────────

    #[test]
    fn lockout_is_none_within_the_free_allowance() {
        for attempts in 0..FREE_UNLOCK_ATTEMPTS {
            assert_eq!(
                lockout_for(attempts),
                None,
                "{} attempts must not lock out",
                attempts
            );
        }
    }

    #[test]
    fn lockout_escalates_then_caps() {
        // First over-threshold failure = 15s, doubling each further failure.
        assert_eq!(lockout_for(5), Some(Duration::from_secs(15)));
        assert_eq!(lockout_for(6), Some(Duration::from_secs(30)));
        assert_eq!(lockout_for(7), Some(Duration::from_secs(60)));
        assert_eq!(lockout_for(8), Some(Duration::from_secs(120)));
        assert_eq!(lockout_for(9), Some(Duration::from_secs(240)));
        // Capped at 5 minutes from here on, including absurd attempt counts
        // (where the doubling would otherwise overflow the shift).
        assert_eq!(lockout_for(10), Some(Duration::from_secs(300)));
        assert_eq!(lockout_for(1000), Some(Duration::from_secs(300)));
    }

    #[test]
    fn note_failed_unlock_arms_lockout_and_reset_clears_it() {
        let mut s = AppState::new_locked([0u8; SALT_SIZE], 1);
        // The free attempts don't arm a lockout.
        for _ in 0..FREE_UNLOCK_ATTEMPTS - 1 {
            assert!(s.note_failed_unlock().is_none());
        }
        assert!(s.lockout_remaining().is_none());
        // The threshold failure arms it; remaining time is now positive.
        assert!(s.note_failed_unlock().is_some());
        assert!(s.lockout_remaining().is_some());
        assert_eq!(s.failed_attempts, FREE_UNLOCK_ATTEMPTS);
        // A successful unlock clears the counter and the cooldown.
        s.reset_unlock_attempts();
        assert_eq!(s.failed_attempts, 0);
        assert!(s.lockout_remaining().is_none());
    }

    // ── Search ──────────────────────────────────────────────────────────

    #[test]
    fn note_matches_search_is_case_insensitive_over_title_and_body() {
        // Empty query matches everything (search inactive never hides a note).
        assert!(note_matches_search("Title", "Body", ""));
        // Title hit, case-insensitive (caller pre-lowercases the query).
        assert!(note_matches_search("Shopping List", "", "shopping"));
        // Body hit.
        assert!(note_matches_search("", "buy milk and eggs", "milk"));
        // No hit in either field.
        assert!(!note_matches_search(
            "Shopping List",
            "buy milk",
            "passport"
        ));
    }

    #[test]
    fn set_and_clear_search_toggle_is_searching() {
        let mut s = unlocked_state_with_one_note();
        assert!(!s.is_searching());
        s.set_search_query("milk");
        assert!(s.is_searching());
        // All-whitespace reads as not searching even though it's stored.
        s.set_search_query("   ");
        assert!(!s.is_searching());
        s.set_search_query("milk");
        s.clear_search();
        assert!(!s.is_searching());
    }

    #[test]
    fn search_narrows_filtered_ids_over_title_and_body() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // Give the two notes distinct, searchable content.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].title = "Groceries".into();
            notes[0].body = "buy milk".into();
            notes[1].title = "Meeting".into();
            notes[1].body = "agenda".into();
        }
        // No query → both, in stored order.
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
        // Title match, case-insensitive.
        s.set_search_query("GROCER");
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1]);
        // Body match on the other note.
        s.set_search_query("agenda");
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![2]);
        // No match → empty, but note_count still ignores the query.
        s.set_search_query("passport");
        assert!(s.filtered_note_ids(SortMode::Created).is_empty());
        assert_eq!(s.note_count(), 2);
    }

    #[test]
    fn search_and_tag_filter_intersect() {
        let mut s = unlocked_state_with_two_tagged_notes();
        // Note 1 = tag "work"; note 2 = tag "personal". Both bodies mention
        // "report" so the search alone wouldn't disambiguate them.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].body = "quarterly report".into();
            notes[1].body = "expense report".into();
        }
        s.set_search_query("report");
        // Search alone → both.
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1, 2]);
        // Add a tag filter → intersection with the search.
        s.toggle_filter_tag("work");
        assert_eq!(s.filtered_note_ids(SortMode::Created), vec![1]);
        // A query that misses the tag-matching note empties the intersection.
        s.set_search_query("expense");
        assert!(s.filtered_note_ids(SortMode::Created).is_empty());
    }

    // ── Import (add_note) ───────────────────────────────────────────────

    #[test]
    fn add_note_appends_selects_and_marks_dirty() {
        let mut s = unlocked_state_with_one_note();
        let next_before = s.next_id;
        let id = s
            .add_note("Imported".into(), "from a file".into())
            .expect("added");
        assert_eq!(id, next_before, "uses the current next_id");
        assert_eq!(s.next_id, next_before + 1, "bumps next_id");

        if let Phase::Unlocked {
            notes,
            selected,
            dirty,
            ..
        } = &s.phase
        {
            let note = notes.iter().find(|n| n.id == id).expect("note present");
            assert_eq!(
                (note.title.as_str(), note.body.as_str()),
                ("Imported", "from a file")
            );
            assert!(note.tags.is_empty(), "imported notes carry no tags");
            assert_eq!(*selected, Some(id), "the new note is selected");
            assert!(dirty.contains(&id), "the new note is dirty for persistence");
        } else {
            panic!("expected Unlocked");
        }
    }

    // ── Attachments ─────────────────────────────────────────────────────

    /// A minimal valid 2x2 PNG, generated in memory. Tests need to *produce*
    /// real image bytes (the app only *consumes* them via `Image::from_bytes`).
    fn tiny_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        for px in img.pixels_mut() {
            *px = image::Rgba([10, 20, 30, 255]);
        }
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode png");
        out
    }

    #[test]
    fn parse_attachment_ref_only_matches_the_internal_scheme() {
        assert_eq!(parse_attachment_ref("knot-img:7"), Some(7));
        assert_eq!(parse_attachment_ref("knot-img:0"), Some(0));
        // Scheme present but no / non-numeric id.
        assert_eq!(parse_attachment_ref("knot-img:"), None);
        assert_eq!(parse_attachment_ref("knot-img:abc"), None);
        // External / unknown srcs never resolve as attachments.
        assert_eq!(parse_attachment_ref("https://example.com/a.png"), None);
        assert_eq!(parse_attachment_ref("file:///x.png"), None);
        assert_eq!(parse_attachment_ref("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn extract_attachment_refs_collects_every_embedded_id() {
        let body = "intro\n![a](knot-img:1)\nmid ![b](knot-img:42), ![c](https://x/y.png)\n![d](knot-img:1)";
        let mut out = HashSet::new();
        extract_attachment_refs(body, &mut out);
        // ids 1 and 42 (1 appears twice -> deduped); the external image
        // contributes nothing.
        let mut got: Vec<AttachmentId> = out.into_iter().collect();
        got.sort_unstable();
        assert_eq!(got, vec![1, 42]);
    }

    #[test]
    fn extract_attachment_refs_ignores_scheme_without_digits() {
        let mut out = HashSet::new();
        extract_attachment_refs("knot-img: then knot-img:x then knot-img:9z", &mut out);
        // Only "knot-img:9z" yields a number (9); bare/alpha forms don't, and
        // the scan never spins on a non-numeric occurrence.
        let got: Vec<AttachmentId> = out.into_iter().collect();
        assert_eq!(got, vec![9]);
    }

    #[test]
    fn add_attachment_then_resolve_round_trips_through_encryption() {
        let mut s = unlocked_state_with_one_note();
        let png = tiny_png();
        let id = s.add_attachment(&png).expect("store attachment");

        // Resolving decrypts + decodes back to the 2x2 image.
        let img = s.resolve_attachment(id).expect("resolve");
        assert_eq!((img.width(), img.height()), (2, 2));

        // A second resolve hits the cache — same Arc, no re-decrypt.
        let again = s.resolve_attachment(id).expect("cached resolve");
        assert!(
            Arc::ptr_eq(&img, &again),
            "the second resolve must return the cached Arc"
        );
    }

    #[test]
    fn add_attachment_rejects_empty_and_oversized() {
        let mut s = unlocked_state_with_one_note();
        assert!(s.add_attachment(&[]).is_none(), "empty input is rejected");
        let huge = vec![0u8; MAX_ATTACHMENT_BYTES + 1];
        assert!(
            s.add_attachment(&huge).is_none(),
            "input past the size cap is rejected"
        );
    }

    #[test]
    fn resolve_missing_or_undecodable_attachment_is_none() {
        let mut s = unlocked_state_with_one_note();
        // No such id.
        assert!(s.resolve_attachment(999).is_none());
        // Stored bytes that aren't a valid image decode to None — the editor
        // validates before storing, but resolve must stay defensive.
        let id = s.add_attachment(b"not actually an image").expect("store");
        assert!(s.resolve_attachment(id).is_none());
    }

    #[test]
    fn prune_deletes_only_unreferenced_attachments() {
        let mut s = unlocked_state_with_one_note();
        let png = tiny_png();
        let keep = s.add_attachment(&png).expect("store keep");
        let _orphan = s.add_attachment(&png).expect("store orphan");

        // Reference only `keep` from the single note body.
        if let Phase::Unlocked { notes, .. } = &mut s.phase {
            notes[0].body = format!("![image](knot-img:{keep})");
        }
        s.prune_orphan_attachments().expect("prune");

        if let Phase::Unlocked { storage, .. } = &s.phase {
            assert_eq!(
                storage.all_attachment_ids().expect("ids"),
                vec![keep],
                "only the referenced attachment survives the sweep"
            );
        } else {
            panic!("expected Unlocked");
        }
    }
}
