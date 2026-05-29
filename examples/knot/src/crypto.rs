//! Crypto primitives: envelope encryption + BIP39 recovery.
//!
//! Matches Knot v0.7.0 parameters: Argon2id 64MB / 3 iter / 4 lanes for the
//! password KDF, XChaCha20-Poly1305 for everything sealed.
//!
//! ## Envelope (DEK) scheme
//!
//! The vault is keyed by a random 32-byte **DEK** (data encryption key) that
//! is generated once at setup and never changes. The DEK is what SQLCipher is
//! keyed with and what every note's [`seal`]/[`open`] uses. The user's
//! password never touches the DB directly — instead the DEK is *wrapped*
//! (encrypted) twice and stored on disk:
//!
//! * `dek.enc` — DEK wrapped under the Argon2-derived password KEK
//! * `recovery.enc` — DEK wrapped under a KEK derived from the BIP39 recovery
//!   mnemonic
//!
//! Unlocking unwraps the DEK from `dek.enc`; recovery unwraps it from
//! `recovery.enc`. Changing the password (or recovering) only re-wraps the
//! DEK under a new password — the DB never has to be re-encrypted because the
//! DEK is stable. This is why recovery is possible at all: the password is
//! merely one of two locks on the same key.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use zeroize::Zeroizing;

pub const SALT_SIZE: usize = 32;
pub const KEY_SIZE: usize = 32;
pub const NONCE_SIZE: usize = 24;

pub type MasterKey = Zeroizing<[u8; KEY_SIZE]>;

pub fn derive_key(password: &[u8], salt: &[u8]) -> MasterKey {
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(64 * 1024, 3, 4, Some(KEY_SIZE)).expect("valid argon2 params"),
    );
    let mut key = Zeroizing::new([0u8; KEY_SIZE]);
    argon2
        .hash_password_into(password, salt, key.as_mut())
        .expect("argon2 kdf");
    key
}

pub fn random_salt() -> [u8; SALT_SIZE] {
    let mut salt = [0u8; SALT_SIZE];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// Generate a fresh random data-encryption key. Created once at setup and
/// wrapped under both the password KEK and the recovery KEK; it is the key
/// SQLCipher and every note [`seal`] actually use.
pub fn generate_dek() -> MasterKey {
    let mut dek = Zeroizing::new([0u8; KEY_SIZE]);
    OsRng.fill_bytes(dek.as_mut());
    dek
}

/// Wrap (encrypt) the DEK under `kek`. The on-disk blob is
/// `nonce(24) || ciphertext+tag`, the exact framing [`unwrap_dek`] expects.
pub fn wrap_dek(kek: &MasterKey, dek: &MasterKey) -> Vec<u8> {
    let (nonce, ct) = seal(kek, dek.as_ref());
    let mut out = Vec::with_capacity(NONCE_SIZE + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Reverse of [`wrap_dek`]. Returns `None` when `kek` is wrong (AEAD auth
/// failure — i.e. wrong password / recovery key), the blob is truncated, or
/// the plaintext isn't exactly a 32-byte key.
pub fn unwrap_dek(kek: &MasterKey, wrapped: &[u8]) -> Option<MasterKey> {
    if wrapped.len() < NONCE_SIZE {
        return None;
    }
    let mut nonce = [0u8; NONCE_SIZE];
    nonce.copy_from_slice(&wrapped[..NONCE_SIZE]);
    let pt = Zeroizing::new(open(kek, &nonce, &wrapped[NONCE_SIZE..])?);
    if pt.len() != KEY_SIZE {
        return None;
    }
    let mut dek = Zeroizing::new([0u8; KEY_SIZE]);
    dek.copy_from_slice(&pt);
    Some(dek)
}

pub fn random_nonce() -> [u8; NONCE_SIZE] {
    let mut nonce = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Encrypt `plaintext` under `key` with a fresh nonce. Returns `(nonce, ct)`.
pub fn seal(key: &MasterKey, plaintext: &[u8]) -> ([u8; NONCE_SIZE], Vec<u8>) {
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("32-byte key");
    let nonce_bytes = random_nonce();
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .expect("xchacha20poly1305 encrypt");
    (nonce_bytes, ct)
}

/// Attempt to decrypt `ciphertext`. Returns `None` on auth failure.
pub fn open(key: &MasterKey, nonce: &[u8; NONCE_SIZE], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).ok()?;
    let nonce = XNonce::from_slice(nonce);
    cipher.decrypt(nonce, ciphertext).ok()
}

/// BIP39 recovery key: a 12-word mnemonic the user writes down at setup,
/// which derives (via HKDF-SHA256) the recovery KEK that wraps the DEK in
/// `recovery.enc`. If the password is forgotten, the mnemonic re-derives the
/// KEK, unwraps the DEK, and the user picks a new password.
pub mod recovery {
    use super::{KEY_SIZE, MasterKey};
    use bip39::{Language, Mnemonic};
    use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
    use hkdf::Hkdf;
    use sha2::Sha256;
    use zeroize::Zeroizing;

    /// 128 bits of entropy yields a 12-word English mnemonic.
    const ENTROPY_BYTES: usize = 16;
    /// HKDF `info` domain separator. Identical to Knot v0.7.0 so the
    /// derivation matches the upstream design byte-for-byte.
    const HKDF_INFO: &[u8] = b"knot-recovery-kek-v1";
    /// Word count of a 128-bit-entropy mnemonic. Callers validate against
    /// this before attempting derivation so they can give a precise error.
    pub const WORD_COUNT: usize = 12;

    /// Generate a fresh 12-word recovery mnemonic. Held in a
    /// `Zeroizing<String>` so the phrase is wiped on drop — the caller shows
    /// it once for the user to record, then drops it.
    pub fn generate_mnemonic() -> Zeroizing<String> {
        let mut entropy = Zeroizing::new([0u8; ENTROPY_BYTES]);
        OsRng.fill_bytes(entropy.as_mut());
        // from_entropy_in only errors on an invalid entropy length; 16 bytes
        // is always valid, so this expect is unreachable.
        let mnemonic = Mnemonic::from_entropy_in(Language::English, entropy.as_ref())
            .expect("16 bytes is valid BIP39 entropy");
        Zeroizing::new(mnemonic.to_string())
    }

    /// Derive the recovery KEK from a (user-typed) mnemonic phrase. Returns
    /// `None` if the phrase isn't a valid English BIP39 mnemonic — wrong word
    /// count, an unknown word, or a bad checksum.
    pub fn key_to_kek(phrase: &str) -> Option<MasterKey> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase).ok()?;
        let entropy = Zeroizing::new(mnemonic.to_entropy());
        let hk = Hkdf::<Sha256>::new(None, &entropy);
        let mut kek = Zeroizing::new([0u8; KEY_SIZE]);
        hk.expand(HKDF_INFO, kek.as_mut())
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Some(kek)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_is_twelve_words() {
        let m = recovery::generate_mnemonic();
        assert_eq!(m.split_whitespace().count(), recovery::WORD_COUNT);
    }

    #[test]
    fn kek_derivation_is_deterministic() {
        let m = recovery::generate_mnemonic();
        let a = recovery::key_to_kek(&m).expect("valid mnemonic");
        let b = recovery::key_to_kek(&m).expect("valid mnemonic");
        assert_eq!(a.as_ref(), b.as_ref());
    }

    #[test]
    fn invalid_mnemonic_rejected() {
        assert!(recovery::key_to_kek("not a real mnemonic phrase at all").is_none());
        // Right word count, but "zzzz" isn't in the wordlist.
        assert!(
            recovery::key_to_kek("zzzz ".repeat(12).trim()).is_none(),
            "non-wordlist tokens must fail the checksum/parse"
        );
    }

    #[test]
    fn dek_wrap_round_trips() {
        let dek = generate_dek();
        let salt = random_salt();
        let kek = derive_key(b"correct horse battery staple", &salt);
        let wrapped = wrap_dek(&kek, &dek);
        let unwrapped = unwrap_dek(&kek, &wrapped).expect("right kek unwraps");
        assert_eq!(unwrapped.as_ref(), dek.as_ref());
    }

    #[test]
    fn dek_unwrap_rejects_wrong_kek() {
        let dek = generate_dek();
        let salt = random_salt();
        let right = derive_key(b"right-password", &salt);
        let wrong = derive_key(b"wrong-password", &salt);
        let wrapped = wrap_dek(&right, &dek);
        assert!(unwrap_dek(&wrong, &wrapped).is_none());
    }

    #[test]
    fn recovery_unwraps_same_dek_as_password() {
        // The end-to-end invariant that makes recovery work: the DEK wrapped
        // under the recovery KEK is the *same* DEK wrapped under the password
        // KEK, so recovering and unlocking both yield an identical DB key.
        let dek = generate_dek();
        let salt = random_salt();
        let pw_kek = derive_key(b"my-password", &salt);
        let mnemonic = recovery::generate_mnemonic();
        let rec_kek = recovery::key_to_kek(&mnemonic).expect("valid mnemonic");

        let pw_wrap = wrap_dek(&pw_kek, &dek);
        let rec_wrap = wrap_dek(&rec_kek, &dek);

        let from_pw = unwrap_dek(&pw_kek, &pw_wrap).unwrap();
        let from_rec = unwrap_dek(&rec_kek, &rec_wrap).unwrap();
        assert_eq!(from_pw.as_ref(), from_rec.as_ref());
        assert_eq!(from_pw.as_ref(), dek.as_ref());
    }
}
