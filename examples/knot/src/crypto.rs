//! Crypto primitives shared between lock/unlock and (eventually) save/load.
//!
//! Matches Knot v0.7.0 parameters: Argon2id 64MB / 3 iter / 4 lanes for KDF,
//! XChaCha20-Poly1305 for note content. Storage layer (SQLCipher) is M2
//! scope — for M1 the vault is an in-memory `Vec<EncryptedNote>` that we
//! re-encrypt on every save so the cipher path stays exercised.

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
