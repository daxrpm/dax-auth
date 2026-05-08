use std::path::Path;

use anyhow::{Context, Result};
use dax_runtime::{verify_face, VerifyConfig, VerifyReason};

const PASSPHRASE_ENV: &str = "DAX_VAULT_PASSPHRASE";

pub fn run(
    user: &str,
    vault_path: &Path,
    device: u32,
    detector_path: &Path,
    recognizer_path: &Path,
    liveness_path: &Path,
) -> Result<()> {
    let passphrase = std::env::var(PASSPHRASE_ENV).with_context(|| {
        format!("environment variable `{PASSPHRASE_ENV}` is required to unlock the vault")
    })?;

    let mut config = VerifyConfig::new(
        user,
        vault_path,
        passphrase.as_bytes(),
        detector_path,
        recognizer_path,
        liveness_path,
    );
    config.camera_index = device;

    let outcome = verify_face(&config).context("running face verification")?;

    println!("Detection score : {:.3}", outcome.face_score);
    println!(
        "Liveness        : real={:.4} spoof={:.4}",
        outcome.liveness_real, outcome.liveness_spoof
    );
    println!(
        "Best match      : template #{} cosine={:.4} (threshold={})",
        outcome.best_template, outcome.best_cosine, config.match_threshold
    );

    match outcome.reason {
        VerifyReason::Match => {
            println!("Verdict         : ✓ MATCH");
            Ok(())
        }
        VerifyReason::LivenessSpoof => {
            println!("Verdict         : ✗ SPOOF (liveness rejected)");
            std::process::exit(2);
        }
        VerifyReason::BelowThreshold => {
            println!("Verdict         : ✗ NO MATCH (below threshold)");
            std::process::exit(2);
        }
    }
}
