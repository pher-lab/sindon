//! SQLCipher persistence for the vault.
//!
//! The DB is page-encrypted by SQLCipher under the Argon2-derived master key,
//! and each entry's secret is *additionally* sealed with a per-row
//! XChaCha20-Poly1305 ciphertext (belt-and-suspenders, like Knot). Site and
//! username are stored in the clear *within* the encrypted DB — they are not the
//! secret, and keeping them plain lets the list render without unsealing every
//! row.
//!
//! On-disk layout (under [`shroud::platform::storage::config_dir`]):
//!
//! ```text
//! config/shroud-vault/
//!   vault.db    — SQLCipher 4 database, page-encrypted under the master key
//!   vault.salt  — 32 raw bytes, the Argon2 salt (needed to derive the key
//!                 *before* the DB can be opened, so it lives beside it)
//! ```
//!
//! Unlike Knot this example keys SQLCipher directly with the password-derived
//! key (no envelope DEK): it has no change-password or recovery flow, which are
//! the features the DEK indirection exists to serve. Those are deliberately out
//! of scope — the example exists to ground list virtualization on a real,
//! persistent, secret-aware app, not to re-implement Knot.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use zeroize::Zeroize;

const APP_NAME: &str = "shroud-vault";
const DB_FILENAME: &str = "vault.db";
const SALT_FILENAME: &str = "vault.salt";

pub const SALT_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 12;

/// One entry as it lives at rest: identifying metadata in the clear (inside the
/// encrypted DB), the secret sealed per-row.
pub struct StoredEntry {
    pub id: i64,
    pub site: String,
    pub username: String,
    pub nonce: [u8; NONCE_SIZE],
    pub ciphertext: Vec<u8>,
}

/// On-disk paths for the vault, resolved once.
pub struct VaultPaths {
    pub db: PathBuf,
    pub salt: PathBuf,
}

impl VaultPaths {
    /// Resolve to `<config>/shroud-vault/`. `None` if the platform can't report
    /// a config dir (at which point the caller can't persist).
    pub fn default_for_app() -> Option<Self> {
        let dir = shroud::platform::storage::config_dir(APP_NAME).ok()?;
        Some(Self {
            db: dir.join(DB_FILENAME),
            salt: dir.join(SALT_FILENAME),
        })
    }

    /// True iff both the DB and its salt exist — a usable vault.
    pub fn exists(&self) -> bool {
        self.db.exists() && self.salt.exists()
    }

    pub fn read_salt(&self) -> Result<[u8; SALT_SIZE], StorageError> {
        let bytes = std::fs::read(&self.salt)?;
        if bytes.len() != SALT_SIZE {
            return Err(StorageError::Corrupt(format!(
                "salt file is {} bytes, expected {SALT_SIZE}",
                bytes.len()
            )));
        }
        let mut out = [0u8; SALT_SIZE];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    pub fn write_salt(&self, salt: &[u8; SALT_SIZE]) -> Result<(), StorageError> {
        if let Some(parent) = self.salt.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.salt, salt)?;
        Ok(())
    }
}

/// Errors from the storage layer. `BadKey` is pulled out of `Sqlite` so the
/// unlock flow can treat it as "wrong password" without parsing SQLite variants.
#[derive(Debug)]
pub enum StorageError {
    /// SQLCipher could not open the DB with the supplied key (wrong password).
    BadKey,
    /// On-disk data is malformed.
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
            Self::Corrupt(s) => write!(f, "vault file corrupted: {s}"),
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// An open SQLCipher connection to the vault. Dropping it releases plaintext
/// access to the DB.
pub struct VaultStorage {
    conn: Connection,
}

impl VaultStorage {
    /// Open (or create) the vault DB at `path`, keyed with `key`. Returns
    /// [`StorageError::BadKey`] when the file exists but the key doesn't open it.
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::apply_key(&conn, key)?;

        // Cheapest possible read; SQLCipher returns NotADatabase on a wrong key.
        // A freshly created file passes too (cipher header is written after the
        // key PRAGMA), so this doubles as the create path.
        let probe: rusqlite::Result<i64> =
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0));
        match probe {
            Ok(_) => {}
            Err(e) if is_bad_key_error(&e) => return Err(StorageError::BadKey),
            Err(e) => return Err(StorageError::Sqlite(e)),
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                 id INTEGER PRIMARY KEY,
                 site TEXT NOT NULL,
                 username TEXT NOT NULL,
                 nonce BLOB NOT NULL,
                 ciphertext BLOB NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    /// Feed the 32-byte key to SQLCipher as raw key material (`x'<hex>'`), so it
    /// skips its own PBKDF2 — we already ran Argon2. The hex is zeroized right
    /// after the PRAGMA so it lives for the shortest possible window.
    fn apply_key(conn: &Connection, key: &[u8; 32]) -> Result<(), StorageError> {
        let mut hex = String::with_capacity(64);
        for b in key.iter() {
            write!(&mut hex, "{b:02x}").ok();
        }
        let stmt = format!("PRAGMA key = \"x'{hex}'\";");
        let result = conn.execute_batch(&stmt);
        hex.zeroize();
        result?;
        Ok(())
    }

    /// Load every entry, ordered by id.
    pub fn load_entries(&self) -> Result<Vec<StoredEntry>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, site, username, nonce, ciphertext FROM entries ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, site, username, nonce_blob, ciphertext) = r?;
            if nonce_blob.len() != NONCE_SIZE {
                return Err(StorageError::Corrupt(format!(
                    "entry {id} has nonce length {} (expected {NONCE_SIZE})",
                    nonce_blob.len()
                )));
            }
            let mut nonce = [0u8; NONCE_SIZE];
            nonce.copy_from_slice(&nonce_blob);
            out.push(StoredEntry {
                id,
                site,
                username,
                nonce,
                ciphertext,
            });
        }
        Ok(out)
    }

    /// Replace the whole table in one transaction (used for the first-run seed).
    pub fn save_all(&mut self, entries: &[StoredEntry]) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM entries", [])?;
        for e in entries {
            tx.execute(
                "INSERT INTO entries (id, site, username, nonce, ciphertext)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.id, e.site, e.username, &e.nonce[..], &e.ciphertext],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// SQLCipher returns SQLITE_NOTADB when the key fails to decrypt the page
/// header. Matched specifically so callers see `BadKey` (= wrong password).
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

    fn tmp_db() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "shroud-vault-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn entry(id: i64) -> StoredEntry {
        StoredEntry {
            id,
            site: format!("site{id}"),
            username: format!("user{id}"),
            nonce: [id as u8; NONCE_SIZE],
            ciphertext: vec![id as u8; 24],
        }
    }

    #[test]
    fn round_trips_entries_across_reopen() {
        // Save, drop the connection, reopen with the same key: the rows must
        // survive. Catches forgotten commits / in-memory-only regressions.
        let path = tmp_db();
        let key = [7u8; 32];
        {
            let mut store = VaultStorage::open(&path, &key).expect("open fresh");
            store
                .save_all(&[entry(1), entry(2), entry(3)])
                .expect("save");
        }
        let store = VaultStorage::open(&path, &key).expect("reopen");
        let loaded = store.load_entries().expect("load");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].id, 1);
        assert_eq!(loaded[0].site, "site1");
        assert_eq!(loaded[1].username, "user2");
        assert_eq!(loaded[2].ciphertext, vec![3u8; 24]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_key_returns_bad_key() {
        // SQLCipher can't open a DB keyed with a different key — the
        // wrong-password signal, surfaced as `BadKey` before any row is read.
        let path = tmp_db();
        {
            let mut store = VaultStorage::open(&path, &[1u8; 32]).expect("open with key a");
            store.save_all(&[entry(1)]).expect("save");
        }
        let result = VaultStorage::open(&path, &[2u8; 32]);
        assert!(
            matches!(result, Err(StorageError::BadKey)),
            "a wrong key must surface as BadKey, got {:?}",
            result.err()
        );
        let _ = std::fs::remove_file(&path);
    }
}
