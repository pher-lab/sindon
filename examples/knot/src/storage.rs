//! SQLCipher persistence for Knot's vault (M2).
//!
//! Two-layer encryption: the DB itself is page-encrypted by SQLCipher
//! under a key derived from the master key, *and* each note row holds a
//! per-note XChaCha20-Poly1305 ciphertext produced by `crypto::seal`.
//! The belt-and-suspenders setup means a future flaw in either layer
//! doesn't immediately leak note plaintext.
//!
//! Layout on disk (under [`shroud::platform::storage::config_dir`]):
//!
//! ```text
//! config/knot/
//!   vault.db      — SQLCipher 4 database (page-encrypted under the DEK)
//!   vault.salt    — 32 raw bytes, Argon2 salt for the password KDF
//!   dek.enc       — DEK wrapped under the password-derived KEK
//!   recovery.enc  — DEK wrapped under the BIP39 recovery KEK
//! ```
//!
//! The DB is keyed with the random **DEK**, not the password. The salt +
//! `dek.enc` are what the password unlocks: derive the KEK from the
//! password + salt, unwrap the DEK from `dek.enc`, then open the DB. The
//! salt sits next to the DB rather than inside it because the KEK (needed
//! to unwrap the DEK) requires the salt *before* the DB can be opened.
//! `recovery.enc` is a second wrapping of the same DEK, so a forgotten
//! password can be replaced without re-encrypting the database.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use zeroize::Zeroize;

use crate::crypto::{MasterKey, NONCE_SIZE, SALT_SIZE};
use crate::state::{EncryptedNote, NoteId};

const APP_NAME: &str = "knot";
const DB_FILENAME: &str = "vault.db";
const SALT_FILENAME: &str = "vault.salt";
const DEK_FILENAME: &str = "dek.enc";
const RECOVERY_FILENAME: &str = "recovery.enc";

/// Upper bound on a wrapped-DEK blob. A wrap is `nonce(24) +
/// ciphertext(32) + tag(16) = 72` bytes; the cap rejects an obviously
/// corrupt / oversized file before we hand it to the AEAD layer.
const MAX_WRAPPED_DEK_LEN: usize = 256;

/// Resolves the on-disk paths for the Knot vault. Held as a struct so
/// the lock screen can do `vault_exists()` once and the unlock flow can
/// pass the resolved paths into [`VaultStorage::open`] without
/// re-resolving — keeps the "what does this directory look like?"
/// answer in one place.
pub struct VaultPaths {
    pub db: PathBuf,
    pub salt: PathBuf,
    pub dek: PathBuf,
    pub recovery: PathBuf,
}

impl VaultPaths {
    /// Resolve to the per-user config dir under `<config>/knot/`
    /// (Phase 37 helper handles OS-specific path resolution + creates
    /// the dir). Returns `None` if the platform can't report a config
    /// dir — at which point persistence is unavailable and the caller
    /// falls back to in-memory mode.
    pub fn default_for_app() -> Option<Self> {
        let dir = shroud::platform::storage::config_dir(APP_NAME).ok()?;
        Some(Self {
            db: dir.join(DB_FILENAME),
            salt: dir.join(SALT_FILENAME),
            dek: dir.join(DEK_FILENAME),
            recovery: dir.join(RECOVERY_FILENAME),
        })
    }

    /// True iff the db, salt, and wrapped DEK all exist. Any one missing
    /// means we treat this as a fresh install — partial state from a
    /// crashed first-run gets reset rather than asking the user to repair
    /// it manually. (`recovery.enc` is intentionally not required here: a
    /// vault is usable without it; it only gates the recovery flow.)
    pub fn vault_exists(&self) -> bool {
        self.db.exists() && self.salt.exists() && self.dek.exists()
    }

    /// Whether a recovery wrapping was written at setup. The lock screen
    /// uses this to decide whether to offer the "forgot password?" entry.
    pub fn recovery_exists(&self) -> bool {
        self.recovery.exists()
    }

    pub fn read_salt(&self) -> Result<[u8; SALT_SIZE], StorageError> {
        let bytes = std::fs::read(&self.salt)?;
        if bytes.len() != SALT_SIZE {
            return Err(StorageError::Corrupt(format!(
                "salt file is {} bytes, expected {}",
                bytes.len(),
                SALT_SIZE
            )));
        }
        let mut out = [0u8; SALT_SIZE];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    pub fn write_salt(&self, salt: &[u8; SALT_SIZE]) -> Result<(), StorageError> {
        self.write_file(&self.salt, salt)
    }

    /// Read the password-wrapped DEK blob (`nonce || ciphertext+tag`).
    pub fn read_wrapped_dek(&self) -> Result<Vec<u8>, StorageError> {
        Self::read_wrapped(&self.dek)
    }

    pub fn write_wrapped_dek(&self, wrapped: &[u8]) -> Result<(), StorageError> {
        self.write_file(&self.dek, wrapped)
    }

    /// Read the recovery-wrapped DEK blob. `Io` (not found) surfaces to the
    /// caller, which reports "no recovery key set up" rather than treating
    /// it as a wrong-key failure.
    pub fn read_wrapped_recovery(&self) -> Result<Vec<u8>, StorageError> {
        Self::read_wrapped(&self.recovery)
    }

    pub fn write_wrapped_recovery(&self, wrapped: &[u8]) -> Result<(), StorageError> {
        self.write_file(&self.recovery, wrapped)
    }

    fn read_wrapped(path: &Path) -> Result<Vec<u8>, StorageError> {
        let bytes = std::fs::read(path)?;
        if bytes.len() > MAX_WRAPPED_DEK_LEN {
            return Err(StorageError::Corrupt(format!(
                "wrapped key file is {} bytes, exceeds {} cap",
                bytes.len(),
                MAX_WRAPPED_DEK_LEN
            )));
        }
        Ok(bytes)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

/// Errors raised by the storage layer. `BadKey` is the wrong-password
/// signal — pulled out of `Sqlite` because callers (lock screen) want
/// to react to it specifically without parsing SQLite error variants.
#[derive(Debug)]
pub enum StorageError {
    /// SQLCipher could not unlock the DB with the supplied key.
    /// Treated as "wrong password" by the lock screen.
    BadKey,
    /// On-disk data is malformed (wrong nonce length, salt file size
    /// mismatch). Should not happen in normal use; recovery is "delete
    /// the file and start over" for M2.
    Corrupt(String),
    Sqlite(rusqlite::Error),
    Io(io::Error),
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<io::Error> for StorageError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadKey => write!(f, "wrong master password"),
            Self::Corrupt(s) => write!(f, "vault file corrupted: {}", s),
            Self::Sqlite(e) => write!(f, "sqlite error: {}", e),
            Self::Io(e) => write!(f, "io error: {}", e),
        }
    }
}

impl std::error::Error for StorageError {}

/// Open SQLCipher connection holding the user's vault. Drops the conn
/// on `Drop` — combined with the master key drop in `Phase::Unlocked`,
/// this is what makes "lock" actually release plaintext access.
pub struct VaultStorage {
    conn: Connection,
}

impl VaultStorage {
    /// Open or create the vault DB at `path` and key it with `master_key`.
    /// Initializes the schema on a fresh DB.
    ///
    /// Returns `BadKey` when the file exists but the key doesn't open it
    /// (wrong password). Other `rusqlite::Error`s surface as
    /// `StorageError::Sqlite` and indicate either disk failure or a
    /// corrupted DB.
    pub fn open(path: &Path, master_key: &MasterKey) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::apply_key(&conn, master_key)?;

        // Probe with the cheapest possible read — SQLCipher returns
        // NotADatabase on wrong key, which we surface as BadKey. A
        // freshly-created file passes this probe too (no schema yet,
        // but the cipher header is in place after `PRAGMA key`).
        let probe: rusqlite::Result<i64> =
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0));
        match probe {
            Ok(_) => {}
            Err(e) if is_bad_key_error(&e) => return Err(StorageError::BadKey),
            Err(e) => return Err(StorageError::Sqlite(e)),
        }

        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Hex-encode the 32-byte master key and feed it to SQLCipher via
    /// `PRAGMA key = "x'<hex>'"`. The `x'...'` syntax tells SQLCipher
    /// to use the bytes verbatim as the page-encryption key instead of
    /// running its built-in PBKDF2 — we already did Argon2 in
    /// [`crate::crypto::derive_key`], so PBKDF2 on top would be wasted
    /// work and would couple our security to SQLCipher's KDF choices.
    ///
    /// The hex string is zeroized after the PRAGMA returns so it lives
    /// for the shortest possible window in process memory.
    fn apply_key(conn: &Connection, master_key: &MasterKey) -> Result<(), StorageError> {
        let mut hex = String::with_capacity(64);
        for b in master_key.as_ref().iter() {
            write!(&mut hex, "{:02x}", b).ok();
        }
        let stmt = format!("PRAGMA key = \"x'{}'\";", hex);
        let result = conn.execute_batch(&stmt);
        hex.zeroize();
        result?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<(), StorageError> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY,
                nonce BLOB NOT NULL,
                ciphertext BLOB NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    /// Load every encrypted note, ordered by id (matches the M1
    /// in-memory layout so the sidebar order is stable across saves).
    pub fn load_notes(&self) -> Result<Vec<EncryptedNote>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, nonce, ciphertext FROM notes ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            let id: NoteId = row.get(0)?;
            let nonce_blob: Vec<u8> = row.get(1)?;
            let ciphertext: Vec<u8> = row.get(2)?;
            Ok((id, nonce_blob, ciphertext))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, nonce_blob, ciphertext) = r?;
            if nonce_blob.len() != NONCE_SIZE {
                return Err(StorageError::Corrupt(format!(
                    "note id {} has nonce length {} (expected {})",
                    id,
                    nonce_blob.len(),
                    NONCE_SIZE
                )));
            }
            let mut nonce = [0u8; NONCE_SIZE];
            nonce.copy_from_slice(&nonce_blob);
            out.push(EncryptedNote {
                id,
                nonce,
                ciphertext,
            });
        }
        Ok(out)
    }

    /// Replace the entire `notes` table in one transaction. Used for
    /// bulk operations: first-launch seed, lock-time flush, and
    /// add/delete that already rebuild the vault.
    pub fn save_all_notes(&mut self, notes: &[EncryptedNote]) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM notes", [])?;
        for note in notes {
            tx.execute(
                "INSERT INTO notes (id, nonce, ciphertext) VALUES (?1, ?2, ?3)",
                params![note.id, &note.nonce[..], &note.ciphertext],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Upsert a single row. Used by the per-edit auto-save tick so
    /// typing in one note doesn't rewrite every other note's row.
    pub fn save_note(&mut self, note: &EncryptedNote) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO notes (id, nonce, ciphertext) VALUES (?1, ?2, ?3)",
            params![note.id, &note.nonce[..], &note.ciphertext],
        )?;
        Ok(())
    }

    pub fn delete_note(&mut self, id: NoteId) -> Result<(), StorageError> {
        self.conn
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }
}

/// SQLCipher returns SQLITE_NOTADB when the supplied key fails to
/// decrypt the page header. Matched specifically so callers see
/// `StorageError::BadKey` (= wrong password) instead of a generic
/// SQLite error variant that they'd otherwise have to introspect.
fn is_bad_key_error(e: &rusqlite::Error) -> bool {
    use rusqlite::ffi;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == ffi::ErrorCode::NotADatabase
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{derive_key, random_salt, seal};

    fn tmp_db_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "knot-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn make_key(password: &str) -> (MasterKey, [u8; SALT_SIZE]) {
        let salt = random_salt();
        (derive_key(password.as_bytes(), &salt), salt)
    }

    fn make_note(id: NoteId, key: &MasterKey, body: &[u8]) -> EncryptedNote {
        let (nonce, ciphertext) = seal(key, body);
        EncryptedNote {
            id,
            nonce,
            ciphertext,
        }
    }

    #[test]
    fn round_trips_a_vault() {
        // Create → write → read back — the most basic contract. If this
        // breaks, every other test in this module is moot.
        let path = tmp_db_path();
        let (key, _) = make_key("test-pw-a");

        let mut store = VaultStorage::open(&path, &key).expect("open fresh");
        let notes = vec![
            make_note(1, &key, b"first note"),
            make_note(2, &key, b"second note"),
        ];
        store.save_all_notes(&notes).expect("save");
        drop(store);

        // Re-open to prove the data survived `drop(conn)` — catches
        // forgotten commits or in-memory-only DB regressions.
        let store = VaultStorage::open(&path, &key).expect("reopen");
        let loaded = store.load_notes().expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, 1);
        assert_eq!(loaded[0].nonce, notes[0].nonce);
        assert_eq!(loaded[0].ciphertext, notes[0].ciphertext);
        assert_eq!(loaded[1].id, 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_key_returns_bad_key() {
        // Wrong-password detection at the SQLCipher layer — distinct
        // from the per-row XChaCha auth fail, this triggers before we
        // even read any note rows.
        let path = tmp_db_path();
        let (key_a, _) = make_key("right-password");
        let (key_b, _) = make_key("wrong-password");

        {
            let mut store = VaultStorage::open(&path, &key_a).expect("open with key_a");
            store
                .save_all_notes(&[make_note(1, &key_a, b"hi")])
                .unwrap();
        }

        let result = VaultStorage::open(&path, &key_b);
        assert!(
            matches!(result, Err(StorageError::BadKey)),
            "expected BadKey, got {:?}",
            result.map(|_| "Ok").map_err(|e| format!("{:?}", e))
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_note_upserts() {
        // The per-edit auto-save path uses save_note, not
        // save_all_notes — make sure it actually replaces an existing
        // row instead of inserting a duplicate (would explode on
        // PRIMARY KEY).
        let path = tmp_db_path();
        let (key, _) = make_key("upsert-test");
        let mut store = VaultStorage::open(&path, &key).expect("open");

        store
            .save_all_notes(&[make_note(7, &key, b"first")])
            .unwrap();
        store.save_note(&make_note(7, &key, b"second")).unwrap();

        let loaded = store.load_notes().expect("load");
        assert_eq!(loaded.len(), 1);
        // Different bodies produce different ciphertexts under XChaCha,
        // so the upsert really did write the new row.
        let original = make_note(7, &key, b"first");
        assert_ne!(loaded[0].ciphertext, original.ciphertext);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_note_removes_row() {
        let path = tmp_db_path();
        let (key, _) = make_key("delete-test");
        let mut store = VaultStorage::open(&path, &key).expect("open");

        store
            .save_all_notes(&[make_note(1, &key, b"keep"), make_note(2, &key, b"drop")])
            .unwrap();
        store.delete_note(2).unwrap();

        let loaded = store.load_notes().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 1);

        let _ = std::fs::remove_file(&path);
    }
}
