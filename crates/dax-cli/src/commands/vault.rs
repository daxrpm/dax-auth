use std::path::Path;

use anyhow::{bail, Context, Result};
use dax_store::Vault;

use crate::resolve::{resolve, Overrides};

pub fn init(vault_path: Option<&Path>) -> Result<()> {
    let cfg = resolve(Overrides {
        vault: vault_path,
        ..Overrides::default()
    })?;
    if cfg.vault.exists() {
        bail!("vault already exists at {}", cfg.vault.display());
    }
    let vault = Vault::new();
    vault
        .save(&cfg.vault, cfg.passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", cfg.vault.display()))?;
    println!("Empty vault created at {}", cfg.vault.display());
    Ok(())
}

pub fn list(vault_path: Option<&Path>) -> Result<()> {
    let cfg = resolve(Overrides {
        vault: vault_path,
        ..Overrides::default()
    })?;
    let vault = Vault::open(&cfg.vault, cfg.passphrase.as_bytes())
        .with_context(|| format!("opening vault {}", cfg.vault.display()))?;

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

pub fn remove(vault_path: Option<&Path>, user: &str) -> Result<()> {
    let cfg = resolve(Overrides {
        vault: vault_path,
        ..Overrides::default()
    })?;
    let mut vault = Vault::open(&cfg.vault, cfg.passphrase.as_bytes())
        .with_context(|| format!("opening vault {}", cfg.vault.display()))?;
    if !vault.remove_user(user) {
        bail!("user `{user}` not found in vault");
    }
    vault
        .save(&cfg.vault, cfg.passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", cfg.vault.display()))?;
    println!("Removed all templates for `{user}`.");
    Ok(())
}
