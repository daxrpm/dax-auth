//! Encrypted face embedding store.
//!
//! ## Storage layout
//! ```text
//! /var/lib/dax-auth/
//! └── users/
//!     └── {username_hash}/         ← SHA-256 of username (avoids PII in paths)
//!         ├── embeddings.dax       ← AEAD-encrypted embeddings
//!         └── salt.bin             ← 16-byte per-user random salt (reserved, unused in Phase 1)
//! ```
//!
//! ## Encryption
//! - Cipher: ChaCha20-Poly1305 (fast on CPUs without AES-NI)
//! - Key derivation: Argon2id(master_key, sha256(username)) → 32 bytes
//! - Master key: stored in `/etc/dax-auth/master.key` (32 bytes, binary)
//! - Each embeddings file has a unique random 12-byte nonce prepended
//! - Plaintext format: bincode-serialized `Vec<StoredEmbedding>`
//! - AEAD tag provides integrity: tampered files fail to decrypt

use crate::{embedding::FaceEmbedding, CoreError};
use bincode::{config::standard, serde::decode_from_slice, serde::encode_to_vec};
use chacha20poly1305::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::{ZeroizeOnDrop, Zeroizing};

/// Path to the master key file.
const MASTER_KEY_PATH: &str = "/etc/dax-auth/master.key";

/// Name of the embeddings file inside the user directory.
const EMBEDDINGS_FILE: &str = "embeddings.dax";

// ─── Serializable on-disk representation ─────────────────────────────────────

/// Embedding record persisted to disk (plaintext before encryption).
///
/// This is the format stored inside the encrypted `embeddings.dax` file.
/// It deliberately omits security-sensitive metadata that should not outlive
/// a session (raw similarity scores, timestamps of failed attempts, etc.).
#[derive(Serialize, Deserialize)]
struct StoredEmbedding {
    /// Human-readable label (e.g. `"enrolled_2024-01-15"` or `"with_glasses"`).
    label: String,
    /// The embedding values (512 f32s for ArcFace R100).
    values: Vec<f32>,
    /// Unix timestamp (seconds) when this embedding was enrolled.
    enrolled_at: u64,
}

// ─── In-memory types ──────────────────────────────────────────────────────────

/// A collection of face embeddings for a single user.
///
/// `ZeroizeOnDrop` ensures embedding data is zeroed when this struct is dropped,
/// preventing biometric data from lingering in memory.
#[derive(ZeroizeOnDrop)]
pub struct UserEmbeddings {
    /// The stored face embeddings.
    pub embeddings: Vec<FaceEmbedding>,
}

/// Per-user derived encryption key.
///
/// Zeroed on drop via [`Zeroizing`].
struct UserKey {
    key: Zeroizing<[u8; 32]>,
}

// ─── FaceStore ────────────────────────────────────────────────────────────────

/// Encrypted face embedding store.
///
/// Each user's embeddings are stored in a separate directory identified by the
/// SHA-256 hash of the username (so the filesystem path reveals no PII). The
/// embeddings file is encrypted with ChaCha20-Poly1305 using a key derived from
/// the system master key via Argon2id.
pub struct FaceStore {
    /// Base directory containing per-user subdirectories.
    base_dir: PathBuf,
    /// System master key — used to derive per-user encryption keys.
    ///
    /// Zeroed on drop.
    master_key: Zeroizing<[u8; 32]>,
}

impl FaceStore {
    /// Open the face store using the system master key.
    ///
    /// Reads the 32-byte master key from `/etc/dax-auth/master.key`.
    ///
    /// **Phase 1 development fallback**: if the master key file is absent, a
    /// random key is generated and saved to `/tmp/dax-auth-master.key` with a
    /// `warn!` log. This allows testing without a full system installation. In
    /// production the file must exist before starting the daemon.
    ///
    /// # Errors
    /// Returns [`CoreError::Store`] if the key file cannot be read or has the
    /// wrong length.
    pub fn open(base_dir: impl AsRef<Path>) -> Result<Self, CoreError> {
        let base_dir = base_dir.as_ref().to_path_buf();

        let master_key = read_or_generate_master_key()?;

        // Ensure the base directory exists.
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| CoreError::Store(format!("cannot create store dir: {e}")))?;

        Ok(Self {
            base_dir,
            master_key,
        })
    }

    /// Create a `FaceStore` directly from a key — useful in unit tests to avoid
    /// touching the filesystem for the master key file.
    ///
    /// This constructor is intentionally `pub(crate)` so integration tests inside
    /// this crate can use it.  External callers should use [`FaceStore::open`].
    pub fn new_with_key(base_dir: PathBuf, master_key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            base_dir,
            master_key,
        }
    }

    /// Load all embeddings for a user.
    ///
    /// # Errors
    /// - [`CoreError::NoEnrolledFaces`] if no embeddings exist for this user.
    /// - [`CoreError::Store`] on I/O or decryption failure.
    pub fn load(&self, username: &str) -> Result<UserEmbeddings, CoreError> {
        let path = embeddings_path(&self.base_dir, username);

        if !path.exists() {
            return Err(CoreError::NoEnrolledFaces {
                user: username.to_owned(),
            });
        }

        let data =
            std::fs::read(&path).map_err(|e| CoreError::Store(format!("read error: {e}")))?;

        let user_key = derive_user_key(&self.master_key, username);
        let plaintext = decrypt_data(&user_key, &data)?;

        let (stored, _): (Vec<StoredEmbedding>, _) = decode_from_slice(&plaintext, standard())
            .map_err(|e| CoreError::Store(format!("deserialize error: {e}")))?;

        let embeddings = stored
            .into_iter()
            .map(|s| FaceEmbedding { data: s.values })
            .collect();

        tracing::debug!(username_hash = %username_hash(username), "embeddings loaded");

        Ok(UserEmbeddings { embeddings })
    }

    /// Enroll a new face embedding for a user.
    ///
    /// Loads existing embeddings (if any), appends the new one, and writes back
    /// the full set atomically (write to `.dax.tmp` then rename).
    ///
    /// # Errors
    /// Returns [`CoreError::Store`] if the write fails.
    pub fn enroll(&self, username: &str, embedding: FaceEmbedding) -> Result<(), CoreError> {
        let user_dir = user_dir_path(&self.base_dir, username);
        std::fs::create_dir_all(&user_dir)
            .map_err(|e| CoreError::Store(format!("cannot create user dir: {e}")))?;

        // Load existing embeddings (start empty if none).
        // Note: We cannot move out of `UserEmbeddings` (it's `ZeroizeOnDrop`), so
        // we borrow and clone the data vecs — a necessary cost for the security invariant.
        let mut stored: Vec<StoredEmbedding> = match self.load(username) {
            Ok(u) => u
                .embeddings
                .iter()
                .map(stored_embedding_from_face)
                .collect(),
            Err(CoreError::NoEnrolledFaces { .. }) => Vec::new(),
            Err(e) => return Err(e),
        };

        stored.push(stored_embedding_from_face(&embedding));

        self.write_embeddings(username, &user_dir, &stored)
    }

    /// Remove all enrolled faces for a user.
    ///
    /// Removes the entire user directory including all files inside it.
    ///
    /// # Errors
    /// Returns [`CoreError::Store`] if the directory cannot be removed.
    pub fn clear(&self, username: &str) -> Result<(), CoreError> {
        let dir = user_dir_path(&self.base_dir, username);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| CoreError::Store(format!("cannot remove user dir: {e}")))?;
        }
        tracing::debug!(username_hash = %username_hash(username), "enrollments cleared");
        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Serialize, encrypt, and write `stored` to disk atomically.
    fn write_embeddings(
        &self,
        username: &str,
        user_dir: &Path,
        stored: &[StoredEmbedding],
    ) -> Result<(), CoreError> {
        let plaintext =
            encode_to_vec(stored, standard()).map_err(|e| CoreError::Store(e.to_string()))?;

        let user_key = derive_user_key(&self.master_key, username);
        let ciphertext = encrypt_data(&user_key, &plaintext)?;

        // Atomic write: write to temp file then rename.
        let final_path = user_dir.join(EMBEDDINGS_FILE);
        let tmp_path = user_dir.join("embeddings.dax.tmp");

        std::fs::write(&tmp_path, &ciphertext)
            .map_err(|e| CoreError::Store(format!("write error: {e}")))?;

        std::fs::rename(&tmp_path, &final_path)
            .map_err(|e| CoreError::Store(format!("rename error: {e}")))?;

        tracing::debug!(
            username_hash = %username_hash(username),
            count = stored.len(),
            "embeddings written"
        );
        Ok(())
    }
}

// ─── Key derivation ───────────────────────────────────────────────────────────

/// Derive a per-user encryption key from the master key using Argon2id.
///
/// - Password: master key bytes
/// - Salt: SHA-256(username) — 32 bytes
/// - Output: 32-byte key
fn derive_user_key(master_key: &Zeroizing<[u8; 32]>, username: &str) -> UserKey {
    use argon2::{Algorithm, Argon2, Params, Version};

    // Salt = SHA-256(username) — deterministic, username-unique.
    let salt: [u8; 32] = Sha256::digest(username.as_bytes()).into();

    let params = Params::new(
        64 * 1024, // 64 MB memory
        3,         // 3 iterations
        1,         // 1 thread
        Some(32),  // 32-byte output
    )
    // SAFETY: params are compile-time constants — this is a programmer-error path
    .expect("valid Argon2 params");

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(master_key.as_ref(), &salt, key.as_mut())
        // SAFETY: params are compile-time constants — this is a programmer-error path
        .expect("Argon2 hashing failed with valid params");

    UserKey { key }
}

// ─── Encryption / decryption ──────────────────────────────────────────────────

/// Encrypt `plaintext` with ChaCha20-Poly1305 using a fresh random nonce.
///
/// Output format: `[nonce (12 bytes)][ciphertext + AEAD tag]`.
fn encrypt_data(key: &UserKey, plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.key.as_ref()));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CoreError::Store(format!("encryption error: {e}")))?;

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data produced by [`encrypt_data`].
///
/// # Errors
/// - [`CoreError::Store`] if data is too short or decryption/authentication fails.
fn decrypt_data(key: &UserKey, data: &[u8]) -> Result<Vec<u8>, CoreError> {
    if data.len() < 12 {
        return Err(CoreError::Store(
            "encrypted data too short (expected at least 12-byte nonce)".into(),
        ));
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.key.as_ref()));
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CoreError::Store("decryption failed — data tampered or wrong key".into()))
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

/// Compute the SHA-256 hex digest of `username`.
fn username_hash(username: &str) -> String {
    format!("{:x}", Sha256::digest(username.as_bytes()))
}

/// Return the per-user storage directory path.
fn user_dir_path(base_dir: &Path, username: &str) -> PathBuf {
    base_dir.join(username_hash(username))
}

/// Return the path to the encrypted embeddings file for a user.
fn embeddings_path(base_dir: &Path, username: &str) -> PathBuf {
    user_dir_path(base_dir, username).join(EMBEDDINGS_FILE)
}

// ─── StoredEmbedding helpers ──────────────────────────────────────────────────

/// Convert a [`FaceEmbedding`] reference to a [`StoredEmbedding`] for disk persistence.
///
/// We borrow (and clone the data) rather than consuming `FaceEmbedding` because
/// `ZeroizeOnDrop` prevents moving out of the struct's fields.
fn stored_embedding_from_face(embedding: &FaceEmbedding) -> StoredEmbedding {
    use std::time::{SystemTime, UNIX_EPOCH};

    let enrolled_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    StoredEmbedding {
        label: format!(
            "enrolled_{}",
            chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S")
        ),
        values: embedding.data.clone(),
        enrolled_at,
    }
}

// ─── Master key management ────────────────────────────────────────────────────

/// Read the 32-byte master key from `/etc/dax-auth/master.key`.
///
/// **Phase 1 fallback**: if the file is absent, generates a random key and
/// writes it to `/tmp/dax-auth-master.key`, logging a warning. This enables
/// development and testing without a full system installation.
///
/// # Errors
/// Returns [`CoreError::Store`] if the file exists but cannot be read or has
/// the wrong length.
fn read_or_generate_master_key() -> Result<Zeroizing<[u8; 32]>, CoreError> {
    let production_path = Path::new(MASTER_KEY_PATH);

    if production_path.exists() {
        return read_master_key_from(production_path);
    }

    // Phase 1 development fallback.
    tracing::warn!(
        path = MASTER_KEY_PATH,
        "master key file not found — generating ephemeral key for Phase 1 development. \
         This is INSECURE: enrollments will be lost across daemon restarts unless the \
         same key is restored. Create {} for production use.",
        MASTER_KEY_PATH
    );

    let fallback_path = Path::new("/tmp/dax-auth-master.key");

    if fallback_path.exists() {
        return read_master_key_from(fallback_path);
    }

    // Generate and persist a new random key for this development session.
    let mut key_bytes = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key_bytes.as_mut());

    std::fs::write(fallback_path, key_bytes.as_ref())
        .map_err(|e| CoreError::Store(format!("cannot write fallback master key: {e}")))?;

    tracing::info!(
        path = %fallback_path.display(),
        "ephemeral master key written"
    );

    Ok(key_bytes)
}

/// Read a 32-byte master key from the given path.
fn read_master_key_from(path: &Path) -> Result<Zeroizing<[u8; 32]>, CoreError> {
    let bytes = std::fs::read(path)
        .map_err(|e| CoreError::Store(format!("cannot read master key: {e}")))?;

    let key_array: [u8; 32] = bytes.try_into().map_err(|_| {
        CoreError::Store(format!(
            "master key at {} must be exactly 32 bytes",
            path.display()
        ))
    })?;

    Ok(Zeroizing::new(key_array))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::EMBEDDING_DIM;
    use tempfile::TempDir;

    fn make_store(dir: &TempDir) -> FaceStore {
        FaceStore::new_with_key(dir.path().to_owned(), Zeroizing::new([42u8; 32]))
    }

    fn make_embedding(fill: f32) -> FaceEmbedding {
        FaceEmbedding {
            data: vec![fill; EMBEDDING_DIM],
        }
    }

    // ── Encrypt / decrypt ─────────────────────────────────────────────────────

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = derive_user_key(&Zeroizing::new([0u8; 32]), "testuser");
        let plaintext = b"hello, secure world!";
        let encrypted = encrypt_data(&key, plaintext).expect("encrypt");
        let decrypted = decrypt_data(&key, &encrypted).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_tampered_data_fails() {
        let key = derive_user_key(&Zeroizing::new([0u8; 32]), "testuser");
        let mut encrypted = encrypt_data(&key, b"secret payload").expect("encrypt");
        // Tamper with the last byte of the ciphertext (AEAD tag area).
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(
            decrypt_data(&key, &encrypted).is_err(),
            "tampered data should fail to decrypt"
        );
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = derive_user_key(&Zeroizing::new([0u8; 32]), "testuser");
        assert!(decrypt_data(&key, &[0u8; 11]).is_err());
        assert!(decrypt_data(&key, &[]).is_err());
    }

    #[test]
    fn different_keys_cannot_decrypt_each_other() {
        let key_a = derive_user_key(&Zeroizing::new([0u8; 32]), "alice");
        let key_b = derive_user_key(&Zeroizing::new([0u8; 32]), "bob");
        let encrypted = encrypt_data(&key_a, b"alice secret").expect("encrypt");
        assert!(
            decrypt_data(&key_b, &encrypted).is_err(),
            "bob's key should not decrypt alice's data"
        );
    }

    // ── Enroll / load roundtrip ───────────────────────────────────────────────

    #[test]
    fn enroll_and_load_roundtrip() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);
        let embedding = make_embedding(0.1);

        store.enroll("alice", embedding.clone()).expect("enroll");

        assert!(
            embeddings_path(dir.path(), "alice").exists(),
            "embeddings file must exist after enroll"
        );

        let loaded = store.load("alice").expect("load");
        assert_eq!(loaded.embeddings.len(), 1);

        // Verify values are preserved through encrypt → decrypt → deserialize.
        for (a, b) in loaded.embeddings[0].data.iter().zip(embedding.data.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "embedding values should survive round-trip"
            );
        }
    }

    #[test]
    fn enroll_multiple_accumulates() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);

        for i in 0..3_u8 {
            let emb = make_embedding(i as f32 * 0.1 + 0.1);
            store.enroll("bob", emb).expect("enroll");
        }

        let loaded = store.load("bob").expect("load");
        assert_eq!(
            loaded.embeddings.len(),
            3,
            "all 3 embeddings should be stored"
        );
    }

    #[test]
    fn load_returns_no_enrolled_faces_for_unknown_user() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);

        let result = store.load("unknown_user");
        assert!(
            matches!(result, Err(CoreError::NoEnrolledFaces { .. })),
            "unknown user should return NoEnrolledFaces"
        );
    }

    // ── Clear ─────────────────────────────────────────────────────────────────

    #[test]
    fn clear_removes_enrollments() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);

        store
            .enroll("charlie", make_embedding(0.5))
            .expect("enroll");
        assert!(store.load("charlie").is_ok());

        store.clear("charlie").expect("clear");

        let result = store.load("charlie");
        assert!(
            matches!(result, Err(CoreError::NoEnrolledFaces { .. })),
            "cleared user should have no enrollments"
        );
    }

    #[test]
    fn clear_nonexistent_user_is_noop() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);
        // Should not error if user dir doesn't exist.
        store.clear("nobody").expect("clear noop");
    }

    // ── User isolation ────────────────────────────────────────────────────────

    #[test]
    fn different_users_are_isolated() {
        let dir = TempDir::new().expect("tmpdir");
        let store = make_store(&dir);

        store
            .enroll("alice", make_embedding(1.0))
            .expect("enroll alice");
        store
            .enroll("bob", make_embedding(-1.0))
            .expect("enroll bob");

        let alice = store.load("alice").expect("load alice");
        let bob = store.load("bob").expect("load bob");

        assert_eq!(alice.embeddings.len(), 1);
        assert_eq!(bob.embeddings.len(), 1);

        // Their data should differ.
        let sim = alice.embeddings[0].cosine_similarity(&bob.embeddings[0]);
        // fill(1.0) L2-normalized vs fill(-1.0) L2-normalized → cosine = -1.0
        assert!(
            sim < 0.0,
            "different users should have different embeddings, sim={sim}"
        );
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    #[test]
    fn username_hash_is_deterministic() {
        let h1 = username_hash("alice");
        let h2 = username_hash("alice");
        assert_eq!(h1, h2);
    }

    #[test]
    fn username_hash_differs_per_user() {
        assert_ne!(username_hash("alice"), username_hash("bob"));
    }

    #[test]
    fn user_dir_does_not_contain_plaintext_username() {
        let base = PathBuf::from("/var/lib/dax-auth/users");
        let dir = user_dir_path(&base, "sensitiveuser");
        let dir_str = dir.to_string_lossy();
        assert!(
            !dir_str.contains("sensitiveuser"),
            "user dir should not expose plaintext username, got: {dir_str}"
        );
    }

    // ── Key derivation ────────────────────────────────────────────────────────

    #[test]
    fn key_derivation_is_deterministic() {
        let master = Zeroizing::new([7u8; 32]);
        let k1 = derive_user_key(&master, "alice");
        let k2 = derive_user_key(&master, "alice");
        assert_eq!(
            k1.key.as_ref(),
            k2.key.as_ref(),
            "same inputs must produce same key"
        );
    }

    #[test]
    fn key_derivation_differs_per_user() {
        let master = Zeroizing::new([7u8; 32]);
        let ka = derive_user_key(&master, "alice");
        let kb = derive_user_key(&master, "bob");
        assert_ne!(
            ka.key.as_ref(),
            kb.key.as_ref(),
            "different users must have different derived keys"
        );
    }
}
