use std::path::Path;

use anyhow::{Context, Result, bail};
use dax_capture::{Camera, PixelFormat};
use image::{ImageBuffer, Rgb};
use tracing::info;

pub fn run(device: u32, out: &Path) -> Result<()> {
    let mut camera =
        Camera::open(device).with_context(|| format!("opening camera index {device}"))?;

    let frame = camera
        .capture()
        .with_context(|| format!("capturing frame from device {device}"))?;

    if frame.format() != PixelFormat::Rgb8 {
        bail!(
            "unexpected frame format: {:?}, RGB8 expected for snap",
            frame.format()
        );
    }

    let image: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(frame.width(), frame.height(), frame.data().to_vec())
            .context("frame buffer did not match declared dimensions")?;

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }

    image
        .save(out)
        .with_context(|| format!("writing image to {}", out.display()))?;

    info!(
        path = %out.display(),
        width = frame.width(),
        height = frame.height(),
        "snapshot saved"
    );
    println!(
        "Saved {}x{} snapshot to {}",
        frame.width(),
        frame.height(),
        out.display()
    );
    Ok(())
}
