//! Encrypted vault for biometric templates.
//!
//! On-disk layout:
//!
//! ```text
//! ┌──────────┬───────┬────────┬──────────┬──────────────────────┐
//! │  MAGIC   │ VER u8│ SALT   │ NONCE    │ CIPHERTEXT (JSON+tag)│
//! │ 8 bytes  │ 1 byte│ 16 B   │ 12 B     │ variable length      │
//! └──────────┴───────┴────────┴──────────┴──────────────────────┘
//! ```
//!
//! - `MAGIC = b"DAXVLT01"` (changes with breaking format updates)
//! - Argon2id derives a 32-byte key from the passphrase + salt
//! - `ChaCha20-Poly1305` encrypts and authenticates the JSON body
//! - The plaintext schema is `VaultData` with a versioned `users`
//!   map, allowing additive migrations without breaking older files.

mod crypto;
mod error;
mod vault;

pub use error::{StoreError, StoreResult};
pub use vault::{Template, Vault};
