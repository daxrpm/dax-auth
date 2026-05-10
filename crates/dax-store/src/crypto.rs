use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroize;

use crate::error::{StoreError, StoreResult};

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;

/// Derive a 32-byte key from a passphrase using Argon2id.
///
/// Parameters track the RFC 9106 baseline for second-recommended
/// option (data-independent memory access is irrelevant for our
/// threat model, but cost matters): 64 MiB of memory, 3 iterations,
/// 4 lanes of parallelism. ~100 ms on a modern laptop, hard to
/// brute-force even with a stolen vault.
pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> StoreResult<[u8; KEY_LEN]> {
    let params = Params::new(64 * 1024, 3, 4, Some(KEY_LEN))
        .map_err(|e| StoreError::KeyDerivation(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase, salt, &mut key)
        .map_err(|e| StoreError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// Encrypt `plaintext` with `key` using `ChaCha20-Poly1305` AEAD.
pub fn encrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> StoreResult<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .map_err(|e| StoreError::Encrypt(e.to_string()))
}

/// Decrypt `ciphertext` produced by [`encrypt`].
pub fn decrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
) -> StoreResult<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|e| StoreError::Decrypt(e.to_string()))
}

/// Wipe a key buffer in place.
pub fn wipe_key(key: &mut [u8; KEY_LEN]) {
    key.zeroize();
}
