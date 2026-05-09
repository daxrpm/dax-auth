use std::path::PathBuf;

use anyhow::{Context, Result};
use dax_runtime::{verify_face, VerifyConfig, VerifyReason};

use crate::resolve::{default_user, resolve, Overrides};

#[derive(Debug)]
pub struct Args {
    pub user: Option<String>,
    pub vault: Option<PathBuf>,
    pub device: Option<u32>,
    pub detector: Option<PathBuf>,
    pub recognizer: Option<PathBuf>,
    pub liveness_model: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let user = match args.user {
        Some(u) => u,
        None => default_user().context("--user not provided and could not be inferred")?,
    };

    let cfg = resolve(Overrides {
        vault: args.vault.as_deref(),
        detector: args.detector.as_deref(),
        recognizer: args.recognizer.as_deref(),
        liveness: args.liveness_model.as_deref(),
        camera_index: args.device,
    })?;

    let mut verify_cfg = VerifyConfig::new(
        &user,
        &cfg.vault,
        cfg.passphrase.as_bytes(),
        &cfg.detector,
        &cfg.recognizer,
        &cfg.liveness,
    );
    verify_cfg.camera_index = cfg.camera_index;

    let outcome = verify_face(&verify_cfg).context("running face verification")?;

    println!("Detection score : {:.3}", outcome.face_score);
    println!(
        "Liveness        : real={:.4} spoof={:.4}",
        outcome.liveness_real, outcome.liveness_spoof
    );
    println!(
        "Best match      : template #{} cosine={:.4} (threshold={})",
        outcome.best_template, outcome.best_cosine, verify_cfg.match_threshold
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
