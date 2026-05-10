use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroize;

use crate::error::{StoreError, StoreResult};

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;

/// Argon2id cost parameters used to derive the file's encryption key.
/// Stored alongside the salt and nonce so a future tightening (or
/// loosening) of the defaults does not invalidate existing vaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    #[must_use]
    pub const fn new(m_cost_kib: u32, t_cost: u32, p_cost: u32) -> Self {
        Self {
            m_cost_kib,
            t_cost,
            p_cost,
        }
    }
}

/// Defaults for newly-created vaults. Tracks the RFC 9106 baseline:
/// 64 MiB memory, 3 iterations, 4 lanes — roughly 100 ms on a modern
/// laptop and a steep wall against brute-force on a stolen vault.
pub const DEFAULT_PARAMS: KdfParams = KdfParams::new(64 * 1024, 3, 4);

/// Cost parameters that the original `DAXVLT01` files were written
/// with. Kept here so existing vaults keep decrypting after the
/// defaults are tightened. Not used for new writes.
pub const LEGACY_V1_PARAMS: KdfParams = KdfParams::new(19 * 1024, 2, 1);

/// Derive a 32-byte key from a passphrase using Argon2id.
pub fn derive_key(passphrase: &[u8], salt: &[u8], params: KdfParams) -> StoreResult<[u8; KEY_LEN]> {
    let argon_params = Params::new(
        params.m_cost_kib,
        params.t_cost,
        params.p_cost,
        Some(KEY_LEN),
    )
    .map_err(|e| StoreError::KeyDerivation(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
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
