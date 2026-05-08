use std::path::Path;

use anyhow::{Context, Result};
use dax_detect::Detector;
use dax_liveness::{LivenessChecker, LivenessVerdict};

use crate::commands::embed::{pick_top_face, read_frame};

pub fn run(detector_path: &Path, liveness_path: &Path, input: &Path) -> Result<()> {
    let mut detector = Detector::from_file(detector_path)
        .with_context(|| format!("loading detector {}", detector_path.display()))?;
    let mut checker = LivenessChecker::from_file(liveness_path)
        .with_context(|| format!("loading liveness model {}", liveness_path.display()))?;

    let frame = read_frame(input)?;
    let face = pick_top_face(detector.detect(&frame).context("detection")?, input)?;

    let report = checker
        .check(&frame, &face.bbox)
        .context("running liveness check")?;

    let icon = match report.verdict {
        LivenessVerdict::Real => "LIVE",
        LivenessVerdict::Fake => "SPOOF",
    };
    println!("Verdict      : {icon}");
    println!("Score        : {:.4}", report.score());
    println!(
        "Probabilities: real={:.4} fake={:.4}",
        report.real_prob, report.fake_prob
    );
    println!("Detection    : score={:.3}", face.score);
    Ok(())
}
