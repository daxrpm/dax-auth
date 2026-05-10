use thiserror::Error;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("vault file is too short or malformed")]
    Malformed,

    #[error("vault file has wrong magic header")]
    BadMagic,

    #[error("vault file uses unsupported version: {0}")]
    UnsupportedVersion(u8),

    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    #[error("encryption failed: {0}")]
    Encrypt(String),

    #[error("decryption failed (wrong passphrase or tampered file): {0}")]
    Decrypt(String),

    #[error("serialisation failed: {0}")]
    Serde(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
