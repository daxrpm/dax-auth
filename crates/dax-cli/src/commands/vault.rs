use std::path::Path;

use anyhow::{bail, Context, Result};
use dax_store::Vault;

const PASSPHRASE_ENV: &str = "DAX_VAULT_PASSPHRASE";

pub fn init(vault_path: &Path) -> Result<()> {
    if vault_path.exists() {
        bail!("vault already exists at {}", vault_path.display());
    }
    let passphrase = read_passphrase()?;
    let vault = Vault::new();
    vault
        .save(vault_path, passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", vault_path.display()))?;
    println!("Empty vault created at {}", vault_path.display());
    Ok(())
}

pub fn list(vault_path: &Path) -> Result<()> {
    let passphrase = read_passphrase()?;
    let vault = Vault::open(vault_path, passphrase.as_bytes())
        .with_context(|| format!("opening vault {}", vault_path.display()))?;

    let users = vault.list_users();
    if users.is_empty() {
        println!("Vault is empty.");
        return Ok(());
    }
    println!("Enrolled users ({}):\n", users.len());
    for user in users {
        let count = vault
            .templates_for(user)
            .map_or(0, <[dax_store::Template]>::len);
        println!("  {user:<24} templates={count}");
    }
    Ok(())
}

pub fn remove(vault_path: &Path, user: &str) -> Result<()> {
    let passphrase = read_passphrase()?;
    let mut vault = Vault::open(vault_path, passphrase.as_bytes())
        .with_context(|| format!("opening vault {}", vault_path.display()))?;
    if !vault.remove_user(user) {
        bail!("user `{user}` not found in vault");
    }
    vault
        .save(vault_path, passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", vault_path.display()))?;
    println!("Removed all templates for `{user}`.");
    Ok(())
}

fn read_passphrase() -> Result<String> {
    std::env::var(PASSPHRASE_ENV).with_context(|| {
        format!("environment variable `{PASSPHRASE_ENV}` is required to unlock the vault")
    })
}
