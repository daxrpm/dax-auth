use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::crypto::{
    decrypt, derive_key, encrypt, wipe_key, KdfParams, DEFAULT_PARAMS, LEGACY_V1_PARAMS, NONCE_LEN,
    SALT_LEN,
};
use crate::error::{StoreError, StoreResult};

/// Original on-disk format. Layout:
/// `MAGIC | VERSION(1) | SALT(16) | NONCE(12) | CIPHERTEXT`
/// KDF parameters were hard-coded (19 MiB / 2 / 1) and not stored
/// in the file. Read-only at this point: new writes always emit V2.
const MAGIC_V1: &[u8; 8] = b"DAXVLT01";

/// Current on-disk format. Layout:
/// `MAGIC | VERSION(1) | M_COST_KIB(4) | T_COST(4) | P_COST(4) | SALT(16) | NONCE(12) | CIPHERTEXT`
/// Encoding the Argon2 parameters in the header lets us tighten the
/// defaults without invalidating existing files.
const MAGIC_V2: &[u8; 8] = b"DAXVLT02";
const KDF_PARAMS_LEN: usize = 12;

/// Plaintext schema version. Independent from `MAGIC`; allows minor
/// schema additions without rewriting the on-disk header.
const SCHEMA_VERSION: u8 = 1;

/// One enrolled face template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Template {
    /// L2-normalised embedding (typically 512 floats).
    pub embedding: Vec<f32>,
    /// Unix timestamp (seconds) when the template was enrolled.
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VaultData {
    version: u8,
    users: BTreeMap<String, Vec<Template>>,
}

/// In-memory representation of an encrypted on-disk vault.
#[derive(Debug, Clone)]
pub struct Vault {
    data: VaultData,
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl Vault {
    /// Create a new, empty vault.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: VaultData {
                version: SCHEMA_VERSION,
                users: BTreeMap::new(),
            },
        }
    }

    /// Load and decrypt a vault from disk.
    ///
    /// Both `DAXVLT01` (legacy) and `DAXVLT02` (current, with KDF
    /// parameters in the header) layouts are accepted. Saves always
    /// emit the current layout; an older file silently migrates the
    /// next time the caller persists changes.
    pub fn open(path: impl AsRef<Path>, passphrase: &[u8]) -> StoreResult<Self> {
        let raw = fs::read(path.as_ref())?;
        if raw.len() < MAGIC_V1.len() + 1 {
            return Err(StoreError::Malformed);
        }
        let mut cursor = MAGIC_V1.len();
        let magic = &raw[..cursor];

        let params = if magic == MAGIC_V1 {
            LEGACY_V1_PARAMS
        } else if magic == MAGIC_V2 {
            // V2 reserves 12 bytes for the KDF parameters between
            // VERSION and SALT.
            if raw.len() < cursor + 1 + KDF_PARAMS_LEN + SALT_LEN + NONCE_LEN {
                return Err(StoreError::Malformed);
            }
            // Skip VERSION first, then read params.
            let v_cursor = cursor + 1;
            let m = u32::from_le_bytes(
                raw[v_cursor..v_cursor + 4]
                    .try_into()
                    .map_err(|_| StoreError::Malformed)?,
            );
            let t = u32::from_le_bytes(
                raw[v_cursor + 4..v_cursor + 8]
                    .try_into()
                    .map_err(|_| StoreError::Malformed)?,
            );
            let p = u32::from_le_bytes(
                raw[v_cursor + 8..v_cursor + 12]
                    .try_into()
                    .map_err(|_| StoreError::Malformed)?,
            );
            KdfParams::new(m, t, p)
        } else {
            return Err(StoreError::BadMagic);
        };

        // VERSION
        let version = raw[cursor];
        cursor += 1;
        if version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedVersion(version));
        }
        // Skip the V2 KDF block if present.
        if magic == MAGIC_V2 {
            cursor += KDF_PARAMS_LEN;
        }
        // SALT + NONCE + CIPHERTEXT
        if raw.len() < cursor + SALT_LEN + NONCE_LEN {
            return Err(StoreError::Malformed);
        }
        let salt = <[u8; SALT_LEN]>::try_from(&raw[cursor..cursor + SALT_LEN])
            .map_err(|_| StoreError::Malformed)?;
        cursor += SALT_LEN;
        let nonce = <[u8; NONCE_LEN]>::try_from(&raw[cursor..cursor + NONCE_LEN])
            .map_err(|_| StoreError::Malformed)?;
        cursor += NONCE_LEN;
        let ciphertext = &raw[cursor..];

        let mut key = derive_key(passphrase, &salt, params)?;
        let plaintext = decrypt(&key, &nonce, ciphertext);
        wipe_key(&mut key);
        let plaintext = plaintext?;

        let data: VaultData =
            serde_json::from_slice(&plaintext).map_err(|e| StoreError::Serde(e.to_string()))?;
        debug!(users = data.users.len(), magic = ?std::str::from_utf8(magic).unwrap_or("?"), "vault opened");
        Ok(Self { data })
    }

    /// Encrypt and write the vault to disk atomically (via a sibling
    /// temporary file followed by `rename`).
    pub fn save(&self, path: impl AsRef<Path>, passphrase: &[u8]) -> StoreResult<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let plaintext =
            serde_json::to_vec(&self.data).map_err(|e| StoreError::Serde(e.to_string()))?;

        let mut salt = [0u8; SALT_LEN];
        let mut nonce = [0u8; NONCE_LEN];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut salt);
        rng.fill_bytes(&mut nonce);

        let params = DEFAULT_PARAMS;
        let mut key = derive_key(passphrase, &salt, params)?;
        let ciphertext = encrypt(&key, &nonce, &plaintext);
        wipe_key(&mut key);
        let ciphertext = ciphertext?;

        let tmp_path = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(MAGIC_V2)?;
            file.write_all(&[SCHEMA_VERSION])?;
            file.write_all(&params.m_cost_kib.to_le_bytes())?;
            file.write_all(&params.t_cost.to_le_bytes())?;
            file.write_all(&params.p_cost.to_le_bytes())?;
            file.write_all(&salt)?;
            file.write_all(&nonce)?;
            file.write_all(&ciphertext)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, path)?;
        info!(path = %path.display(), users = self.data.users.len(), "vault saved");
        Ok(())
    }

    /// Append a template to the user's record. Creates the user if
    /// absent.
    pub fn add_template(&mut self, user: &str, embedding: Vec<f32>) {
        let template = Template {
            embedding,
            created_at: now_unix(),
        };
        self.data
            .users
            .entry(user.to_string())
            .or_default()
            .push(template);
    }

    /// List enrolled usernames in sorted order.
    pub fn list_users(&self) -> Vec<&str> {
        self.data.users.keys().map(String::as_str).collect()
    }

    /// Templates for a given user, if any.
    pub fn templates_for(&self, user: &str) -> Option<&[Template]> {
        self.data.users.get(user).map(Vec::as_slice)
    }

    /// Remove all templates for a user. Returns whether the user was
    /// present.
    pub fn remove_user(&mut self, user: &str) -> bool {
        self.data.users.remove(user).is_some()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        dir.join(format!(
            "dax-vault-test-{}-{}.bin",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn roundtrip_preserves_templates() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        let mut vault = Vault::new();
        vault.add_template("alice", vec![0.1, 0.2, 0.3]);
        vault.add_template("alice", vec![0.4, 0.5, 0.6]);
        vault.add_template("bob", vec![-0.1, -0.2, -0.3]);
        vault.save(&path, b"correct horse battery staple").unwrap();

        let loaded = Vault::open(&path, b"correct horse battery staple").unwrap();
        assert_eq!(loaded.list_users(), vec!["alice", "bob"]);
        let alice = loaded.templates_for("alice").unwrap();
        assert_eq!(alice.len(), 2);
        assert_eq!(alice[0].embedding, vec![0.1, 0.2, 0.3]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let path = temp_path("wrongpass");
        let _ = fs::remove_file(&path);

        let mut vault = Vault::new();
        vault.add_template("alice", vec![1.0, 2.0]);
        vault.save(&path, b"original").unwrap();

        let result = Vault::open(&path, b"wrong");
        assert!(matches!(result, Err(StoreError::Decrypt(_))));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn remove_user_returns_presence() {
        let mut vault = Vault::new();
        vault.add_template("alice", vec![0.0]);
        assert!(vault.remove_user("alice"));
        assert!(!vault.remove_user("alice"));
    }
}
